//! Backend factories, selected at compile time.
//!
//! Without `real-capture` the host streams the synthetic test pattern, so the app
//! runs and demonstrates the full pipeline on any machine. With the feature it
//! captures the real screen via xcap.

use std::sync::Arc;

use session_host::{CapturerFactory, InjectorFactory};

/// Builds capture + input factories sharing a single Wayland portal session.
///
/// Creating the session prompts the user for consent, so this blocks briefly on
/// the runtime. The returned [`wayland_portal::PortalHandle`] must be kept alive
/// for the whole host session (it owns the PipeWire capture and portal session).
#[cfg(feature = "wayland")]
pub fn wayland_backend(
    rt: &tokio::runtime::Handle,
) -> Result<(CapturerFactory, InjectorFactory, Arc<wayland_portal::PortalHandle>), String> {
    let portal = Arc::new(
        rt.block_on(wayland_portal::PortalHandle::new(rt.clone()))
            .map_err(|e| e.to_string())?,
    );
    let for_capture = portal.clone();
    let capturer_factory: CapturerFactory =
        Arc::new(move || Ok(Box::new(for_capture.capturer())));
    let for_input = portal.clone();
    let injector_factory: InjectorFactory = Arc::new(move || Ok(Box::new(for_input.injector())));
    Ok((capturer_factory, injector_factory, portal))
}

pub fn capturer_factory() -> CapturerFactory {
    #[cfg(feature = "real-capture")]
    {
        Arc::new(|| Ok(Box::new(capture::XcapCapturer::new())))
    }
    #[cfg(not(feature = "real-capture"))]
    {
        Arc::new(|| Ok(Box::new(capture::SyntheticCapturer::default())))
    }
}

pub fn injector_factory() -> InjectorFactory {
    Arc::new(|| Ok(Box::new(input::EnigoInjector::new()?)))
}
