use protocol::MonitorInfo;

/// Status events emitted by a running viewer session to the UI.
#[derive(Debug, Clone)]
pub enum ViewerEvent {
    /// A new decoded frame is available in the frame slot.
    FrameReady,
    /// The host's active monitor changed resolution.
    Resized { width_px: u32, height_px: u32 },
    /// The host's monitor list changed.
    MonitorsUpdated(Vec<MonitorInfo>),
    /// The session ended.
    Disconnected(String),
    /// A non-fatal error worth surfacing.
    Error(String),
}
