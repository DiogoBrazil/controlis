//! Screen capture behind a backend-agnostic trait.
//!
//! Two backends implement [`ScreenCapturer`]:
//!
//! - [`SyntheticCapturer`] (always available): renders an animated test pattern
//!   with no display or system libraries, so the whole host->viewer pipeline can
//!   run and be tested headless.
//! - [`XcapCapturer`] (feature `xcap-backend`): real capture via `xcap` (Windows
//!   Graphics Capture/GDI, X11). On Linux it pulls in PipeWire system libraries,
//!   which is why it is opt-in until the Wayland phase.
//!
//! A future Wayland backend (ScreenCast portal + PipeWire) will slot in behind the
//! same trait.

mod synthetic;
#[cfg(feature = "xcap-backend")]
mod xcap_backend;

pub use synthetic::SyntheticCapturer;
#[cfg(feature = "xcap-backend")]
pub use xcap_backend::XcapCapturer;

use codec::RgbaFrame;
use protocol::MonitorInfo;

/// Errors from enumerating monitors or capturing a frame.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no monitors were found")]
    NoMonitors,
    #[error("monitor {0} was not found")]
    MonitorNotFound(u32),
    #[error("capture backend failed: {0}")]
    Backend(String),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
}

/// Captures frames from the host's monitors.
///
/// Implementations are used from a dedicated capture thread; the trait is
/// synchronous because the underlying capture APIs are blocking.
pub trait ScreenCapturer: Send {
    /// Lists the monitors currently available on the host.
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;

    /// Captures a single frame from the given monitor as RGBA.
    fn capture(&mut self, monitor_id: u32) -> Result<RgbaFrame, CaptureError>;
}
