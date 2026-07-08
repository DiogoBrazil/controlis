//! Mouse and keyboard injection behind a backend-agnostic trait.
//!
//! The MVP backend is [`EnigoInjector`] (SendInput on Windows, XTest on X11). A
//! Wayland backend using the RemoteDesktop portal is planned behind the same
//! [`InputInjector`] trait.
//!
//! Pointer coordinates arrive from the wire normalized to `0.0..=1.0`; [`coords`]
//! turns them into physical pixels for the active monitor, which is what keeps
//! DPI and resolution mismatches from producing off-target clicks.

pub mod coords;
mod enigo_backend;

pub use enigo_backend::EnigoInjector;

use protocol::{KeyCode, MouseButton, PointerAction};

/// Errors from injecting input events.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("failed to initialize input backend: {0}")]
    Init(String),
    #[error("failed to inject event: {0}")]
    Inject(String),
}

/// Injects input events into the host's OS.
///
/// Not required to be `Send`: an injector is created and used on a single thread
/// owned by the host session, matching how the underlying OS handles are meant to
/// be used.
pub trait InputInjector {
    /// Moves the pointer to an absolute desktop pixel position.
    fn move_pointer(&mut self, x: i32, y: i32) -> Result<(), InputError>;

    /// Presses or releases a mouse button.
    fn mouse_button(&mut self, button: MouseButton, action: PointerAction) -> Result<(), InputError>;

    /// Scrolls by wheel deltas (positive y = up, positive x = right).
    fn mouse_wheel(&mut self, delta_x: f32, delta_y: f32) -> Result<(), InputError>;

    /// Presses or releases a key.
    fn key(&mut self, key: KeyCode, action: PointerAction) -> Result<(), InputError>;

    /// Releases every key and button this injector currently holds down.
    ///
    /// Called on disconnect (including abrupt drops) so no modifier or button is
    /// left stuck on the host.
    fn release_all(&mut self) -> Result<(), InputError>;
}
