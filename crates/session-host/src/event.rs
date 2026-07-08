/// A pending connection awaiting the host user's decision.
#[derive(Debug, Clone)]
pub struct ConnectionRequest {
    pub peer_addr: String,
    pub fingerprint: Option<String>,
}

/// Events emitted by the host to the UI.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// A fresh session code is available to show the user.
    CodeReady(String),
    /// A viewer authenticated and is awaiting approval.
    ApprovalRequested(ConnectionRequest),
    /// A session became active (streaming + control).
    SessionStarted { peer_addr: String },
    /// The active session ended.
    SessionEnded { reason: String },
    /// A human-readable log line (also suitable for the audit log).
    Log(String),
    /// A non-fatal error worth surfacing.
    Error(String),
}

/// Commands sent from the UI to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCommand {
    /// Approve the pending connection.
    Approve,
    /// Reject the pending connection.
    Reject,
    /// End the active session (or cancel a pending approval).
    Stop,
}
