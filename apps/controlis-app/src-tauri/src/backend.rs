//! Host capture/input backend selection, ported from the old egui app.
//!
//! Without `real-capture` the host streams the synthetic test pattern, so the
//! app runs and demonstrates the full pipeline on any machine. With the feature
//! it captures the real screen via xcap; with `wayland` it prefers the portal
//! (PipeWire capture + EIS input), falling back to the default backend when
//! consent fails.

use std::sync::Arc;

use session_host::{CapturerFactory, InjectorFactory};

/// The capture/input backend picked for this host, surfaced in the UI so a
/// silent fallback (e.g. portal consent denied) is visible to the user.
pub struct BackendChoice {
    pub capturer: CapturerFactory,
    pub injector: InjectorFactory,
    pub keepalive: Option<Box<dyn std::any::Any + Send>>,
    /// Human-readable backend name shown in the host screen.
    pub label: String,
    /// Log line explaining a fallback, when one happened.
    pub fallback: Option<String>,
}

/// Builds capture + input factories sharing a single Wayland portal session.
///
/// Creating the session prompts the user for consent, so this blocks briefly on
/// the runtime. The returned `PortalHandle` must be kept alive for the whole
/// host lifetime (it owns the PipeWire capture and portal session).
#[cfg(feature = "wayland")]
fn wayland_backend(
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

fn capturer_factory() -> CapturerFactory {
    #[cfg(feature = "real-capture")]
    {
        Arc::new(|| Ok(Box::new(capture::XcapCapturer::new())))
    }
    #[cfg(not(feature = "real-capture"))]
    {
        Arc::new(|| Ok(Box::new(capture::SyntheticCapturer::default())))
    }
}

fn injector_factory() -> InjectorFactory {
    Arc::new(|| Ok(Box::new(input::EnigoInjector::new()?)))
}

/// Selects the host backend at compile time. Must be called from a thread that
/// may block (the Wayland path blocks on the portal consent dialog).
pub fn build_backend(handle: &tokio::runtime::Handle) -> BackendChoice {
    #[cfg(feature = "wayland")]
    let fallback: Option<String> = match wayland_backend(handle) {
        Ok((capturer, injector, portal)) => {
            return BackendChoice {
                capturer,
                injector,
                keepalive: Some(Box::new(portal)),
                label: "Wayland portal (PipeWire + EIS)".into(),
                fallback: None,
            };
        }
        Err(e) => {
            tracing::error!("wayland portal unavailable: {e}; using default backend");
            Some(format!("Portal Wayland indisponível ({e}); usando enigo"))
        }
    };
    #[cfg(not(feature = "wayland"))]
    let fallback: Option<String> = None;
    let _ = handle;
    let label = if cfg!(feature = "real-capture") {
        "xcap + enigo (X11/Windows)"
    } else {
        "sintético + enigo (demonstração)"
    };
    BackendChoice {
        capturer: capturer_factory(),
        injector: injector_factory(),
        keepalive: None,
        label: label.into(),
        fallback,
    }
}

/// Best-effort LAN IPv4 for the access code; an `advertised_ip` in config.toml
/// takes precedence (machines with VPNs/virtual adapters may need it).
pub fn detect_lan_ip() -> Option<std::net::Ipv4Addr> {
    match local_ip_address::local_ip() {
        Ok(std::net::IpAddr::V4(ip)) => Some(ip),
        Ok(std::net::IpAddr::V6(ip)) => {
            tracing::warn!("primary local address is IPv6 ({ip}); access codes carry IPv4 only");
            None
        }
        Err(e) => {
            tracing::warn!("could not detect the LAN IP: {e}");
            None
        }
    }
}
