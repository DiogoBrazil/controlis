//! Security primitives for session authentication and abuse resistance.
//!
//! Deliberately small and dependency-light: a session-code generator, a
//! constant-time comparison, and a per-source brute-force limiter. Nothing here
//! touches the network or the OS.

mod code;
mod rate_limit;

pub use code::{verify_code, SessionCode};
pub use rate_limit::{BruteForceGuard, GuardDecision};
