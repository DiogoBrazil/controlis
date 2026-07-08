use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Number of failed attempts before a source is temporarily blocked.
const FAILURES_BEFORE_BLOCK: u32 = 5;
/// Base block duration; doubles for each block beyond the first (capped).
const BASE_BLOCK: Duration = Duration::from_secs(15);
const MAX_BLOCK: Duration = Duration::from_secs(15 * 60);
/// Idle window after which a source's failure history is forgotten.
const HISTORY_TTL: Duration = Duration::from_secs(30 * 60);

/// Verdict returned before processing an auth attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    Allow,
    Blocked { retry_after: Duration },
}

#[derive(Debug, Clone)]
struct Record {
    failures: u32,
    blocks: u32,
    blocked_until: Option<Instant>,
    last_seen: Instant,
}

/// Per-IP brute-force limiter with exponential backoff.
///
/// The generic `now` parameter on each method keeps the type deterministically
/// testable without a real clock.
#[derive(Debug, Default)]
pub struct BruteForceGuard {
    records: HashMap<IpAddr, Record>,
}

impl BruteForceGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides whether an attempt from `source` may proceed at time `now`.
    pub fn check(&mut self, source: IpAddr, now: Instant) -> GuardDecision {
        self.expire(now);
        match self.records.get(&source) {
            Some(record) => match record.blocked_until {
                Some(until) if until > now => GuardDecision::Blocked {
                    retry_after: until - now,
                },
                _ => GuardDecision::Allow,
            },
            None => GuardDecision::Allow,
        }
    }

    /// Records a failed attempt, arming a block once the threshold is crossed.
    pub fn record_failure(&mut self, source: IpAddr, now: Instant) {
        let record = self.records.entry(source).or_insert(Record {
            failures: 0,
            blocks: 0,
            blocked_until: None,
            last_seen: now,
        });
        record.last_seen = now;
        record.failures += 1;
        if record.failures >= FAILURES_BEFORE_BLOCK {
            let shift = record.blocks.min(6);
            let block = BASE_BLOCK
                .saturating_mul(1 << shift)
                .min(MAX_BLOCK);
            record.blocked_until = Some(now + block);
            record.blocks += 1;
            record.failures = 0;
        }
    }

    /// Clears history for a source after a successful authentication.
    pub fn record_success(&mut self, source: IpAddr) {
        self.records.remove(&source);
    }

    fn expire(&mut self, now: Instant) {
        self.records.retain(|_, record| {
            let unblocked = record.blocked_until.map_or(true, |until| until <= now);
            let fresh = now.duration_since(record.last_seen) < HISTORY_TTL;
            fresh || !unblocked
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 0, 5))
    }

    #[test]
    fn allows_until_threshold_then_blocks() {
        let mut guard = BruteForceGuard::new();
        let now = Instant::now();
        for _ in 0..FAILURES_BEFORE_BLOCK {
            assert_eq!(guard.check(ip(), now), GuardDecision::Allow);
            guard.record_failure(ip(), now);
        }
        match guard.check(ip(), now) {
            GuardDecision::Blocked { retry_after } => {
                assert!(retry_after <= BASE_BLOCK && retry_after > Duration::ZERO);
            }
            GuardDecision::Allow => panic!("should be blocked after threshold"),
        }
    }

    #[test]
    fn block_expires_after_duration() {
        let mut guard = BruteForceGuard::new();
        let now = Instant::now();
        for _ in 0..FAILURES_BEFORE_BLOCK {
            guard.record_failure(ip(), now);
        }
        let later = now + BASE_BLOCK + Duration::from_secs(1);
        assert_eq!(guard.check(ip(), later), GuardDecision::Allow);
    }

    #[test]
    fn backoff_grows_on_repeated_blocks() {
        let mut guard = BruteForceGuard::new();
        let mut now = Instant::now();
        // First block.
        for _ in 0..FAILURES_BEFORE_BLOCK {
            guard.record_failure(ip(), now);
        }
        let first = match guard.check(ip(), now) {
            GuardDecision::Blocked { retry_after } => retry_after,
            _ => panic!("expected block"),
        };
        // Wait it out, trigger a second block.
        now += first + Duration::from_secs(1);
        for _ in 0..FAILURES_BEFORE_BLOCK {
            guard.record_failure(ip(), now);
        }
        let second = match guard.check(ip(), now) {
            GuardDecision::Blocked { retry_after } => retry_after,
            _ => panic!("expected block"),
        };
        assert!(second > first, "second block {second:?} should exceed first {first:?}");
    }

    #[test]
    fn success_clears_history() {
        let mut guard = BruteForceGuard::new();
        let now = Instant::now();
        for _ in 0..FAILURES_BEFORE_BLOCK {
            guard.record_failure(ip(), now);
        }
        guard.record_success(ip());
        assert_eq!(guard.check(ip(), now), GuardDecision::Allow);
    }
}
