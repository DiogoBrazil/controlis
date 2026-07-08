//! Wayland host backend via xdg-desktop-portal + PipeWire.
//!
//! A single combined portal session (ScreenCast + RemoteDesktop) provides both the
//! captured frames and the input injection, which is required for correct absolute
//! pointer positioning: `NotifyPointerMotionAbsolute` addresses a specific stream
//! from the same session.
//!
//! The real implementation lives behind the `enabled` feature (it needs PipeWire
//! system libraries). Without the feature this crate exposes only the error type,
//! so `cargo build --workspace` works everywhere.

/// Errors from the Wayland portal backend.
#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    #[error("portal backend was built without the `enabled` feature")]
    NotEnabled,
    #[error("portal request failed: {0}")]
    Portal(String),
    #[error("pipewire error: {0}")]
    PipeWire(String),
    #[error("the user did not grant screen capture / remote control")]
    Denied,
    #[error("no stream was granted by the portal")]
    NoStream,
}

#[cfg(feature = "enabled")]
mod backend;

#[cfg(feature = "enabled")]
pub use backend::{PortalCapturer, PortalHandle, PortalInjector};
