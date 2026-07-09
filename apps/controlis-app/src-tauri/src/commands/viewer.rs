//! Viewer-mode commands: connect with an access code, stream frames to the
//! webview canvas and forward input back to the host.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::{ControlMessage, MonitorInfo};
use rendezvous::SessionId;
use serde::Serialize;
use session_viewer::{FrameSlot, ViewerError, ViewerEvent};
use storage::{KnownHost, Store};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use tokio::sync::mpsc;
use transport::Fingerprint;

use crate::input_map::{to_control_message, UiInputEvent};
use crate::AppEnv;

/// Frame pump cadence. The host targets up to 30 fps; polling a bit faster
/// keeps the extra latency low without burning CPU.
const FRAME_POLL: Duration = Duration::from_millis(15);

/// The active viewer session as seen by the commands (the event pump owns the
/// actual `ViewerHandle`).
pub struct ActiveViewer {
    input: mpsc::UnboundedSender<ControlMessage>,
    stop_frames: Arc<AtomicBool>,
}

impl Drop for ActiveViewer {
    fn drop(&mut self) {
        self.stop_frames.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct ViewerSlot(pub Arc<Mutex<Option<ActiveViewer>>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorUi {
    pub id: u32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub is_primary: bool,
}

impl From<&MonitorInfo> for MonitorUi {
    fn from(m: &MonitorInfo) -> Self {
        Self {
            id: m.id,
            name: m.name.clone(),
            width_px: m.width_px,
            height_px: m.height_px,
            is_primary: m.is_primary,
        }
    }
}

/// Returned by `connect_viewer` once the session is accepted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerInfo {
    pub monitors: Vec<MonitorUi>,
    pub fingerprint: Option<String>,
}

/// Status events streamed to the webview while the session runs. Frames travel
/// on their own binary channel.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ViewerUiEvent {
    Resized { width_px: u32, height_px: u32 },
    MonitorsUpdated { monitors: Vec<MonitorUi> },
    Disconnected { reason: String },
    Error { message: String },
}

#[tauri::command]
pub async fn connect_viewer(
    env: State<'_, AppEnv>,
    slot: State<'_, ViewerSlot>,
    code: String,
    addr_override: Option<String>,
    on_event: Channel<ViewerUiEvent>,
    on_frame: Channel<InvokeResponseBody>,
) -> Result<ViewerInfo, String> {
    drop(slot.0.lock().unwrap().take());

    let store = Store::open(&env.db_path)
        .map_err(|e| format!("armazenamento de confiança indisponível: {e}"))?;
    let target = resolve_target(&code, addr_override.as_deref())?;

    let (handle, peer_key, nickname) =
        match target {
            ResolvedTarget::Lan(target) => {
                let expected = expected_fingerprint(&store, &target.peer_key)?;
                let handle = session_viewer::connect(target.addr, target.canonical_code, expected)
                    .await
                    .map_err(connect_error_text)?;
                (handle, target.peer_key, target.nickname)
            }
            ResolvedTarget::Internet {
                session_id,
                canonical_code,
            } => {
                let rendezvous_url = env.config.rendezvous_url.as_deref().ok_or_else(|| {
                    "rendezvous_url não está configurado no config.toml".to_string()
                })?;
                let relay_url =
                    env.config.relay_url.as_deref().ok_or_else(|| {
                        "relay_url não está configurado no config.toml".to_string()
                    })?;
                let client = rendezvous::Client::new(rendezvous_url)
                    .map_err(|e| format!("configuração de rendezvous inválida: {e}"))?;
                let lookup = client
                    .lookup(session_id)
                    .await
                    .map_err(|e| format!("falha ao consultar rendezvous: {e}"))?;
                let peer_key = format!("rendezvous:{}", session_id.get());
                let expected_identity = format!("iroh:{}", lookup.endpoint.endpoint_id);
                expected_iroh_identity(&store, &peer_key, &expected_identity)?;
                let descriptor = transport::IrohEndpointDescriptor {
                    endpoint_id: lookup.endpoint.endpoint_id,
                    relay_url: lookup.endpoint.relay_url,
                    direct_addrs: lookup.endpoint.direct_addrs,
                };
                let handle = session_viewer::connect_internet(
                    env.iroh_secret_key.clone(),
                    relay_url,
                    descriptor,
                    canonical_code,
                )
                .await
                .map_err(connect_error_text)?;
                (handle, peer_key, format!("Internet {}", session_id.get()))
            }
        };

    if let Some(fingerprint) = handle.fingerprint() {
        store
            .remember_host(&KnownHost {
                peer_key,
                fingerprint: fingerprint.to_string(),
                nickname,
            })
            .map_err(|e| format!("falha ao gravar identidade do host: {e}"))?;
    }

    let info = ViewerInfo {
        monitors: handle.monitors().iter().map(MonitorUi::from).collect(),
        fingerprint: handle.fingerprint().map(|f| f.to_string()),
    };

    let stop_frames = Arc::new(AtomicBool::new(false));
    spawn_frame_pump(handle.frame_slot(), on_frame, stop_frames.clone());

    *slot.0.lock().unwrap() = Some(ActiveViewer {
        input: handle.input_sender(),
        stop_frames: stop_frames.clone(),
    });

    tokio::spawn(pump_events(handle, on_event, stop_frames));
    Ok(info)
}

/// A resolved connection target plus the persistence key used for TOFU pinning.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LanTarget {
    addr: SocketAddr,
    canonical_code: String,
    peer_key: String,
    nickname: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedTarget {
    Lan(LanTarget),
    Internet {
        session_id: SessionId,
        canonical_code: String,
    },
}

/// Turns the typed access code (plus optional manual address) into the
/// connection target and the canonical code to authenticate with.
fn resolve_target(code: &str, addr_override: Option<&str>) -> Result<ResolvedTarget, String> {
    let code = security::ConnectCode::parse(code.trim())
        .map_err(|e| format!("Código inválido ({e}). Confira com quem está no computador host."))?;
    let manual_addr = addr_override.map(str::trim).filter(|s| !s.is_empty());
    let code_addr = match code.target() {
        security::ConnectTarget::Lan(addr) => Some(addr),
        security::ConnectTarget::Rendezvous { id } if manual_addr.is_none() => {
            return Ok(ResolvedTarget::Internet {
                session_id: SessionId::new(id).map_err(|e| e.to_string())?,
                canonical_code: code.as_str().to_string(),
            });
        }
        security::ConnectTarget::Rendezvous { .. } => None,
    };
    let addr = match manual_addr {
        None => code_addr.expect("LAN code has embedded address").into(),
        Some(manual) => manual.parse::<SocketAddr>().map_err(|_| {
            "Endereço manual inválido. Use IP:porta, ex. 192.168.0.10:21118".to_string()
        })?,
    };
    Ok(ResolvedTarget::Lan(LanTarget {
        addr,
        canonical_code: code.as_str().to_string(),
        peer_key: format!("lan:{addr}"),
        nickname: format!("LAN {addr}"),
    }))
}

fn expected_fingerprint(store: &Store, peer_key: &str) -> Result<Option<Fingerprint>, String> {
    let Some(host) = store
        .known_host(peer_key)
        .map_err(|e| format!("falha ao consultar identidade conhecida: {e}"))?
    else {
        return Ok(None);
    };
    host.fingerprint
        .parse::<Fingerprint>()
        .map(Some)
        .map_err(|e| {
            format!(
                "registro de identidade do host está corrompido para {peer_key}: {e}. \
             Remova o registro local antes de tentar novamente."
            )
        })
}

fn expected_iroh_identity(store: &Store, peer_key: &str, actual: &str) -> Result<(), String> {
    let Some(host) = store
        .known_host(peer_key)
        .map_err(|e| format!("falha ao consultar identidade conhecida: {e}"))?
    else {
        return Ok(());
    };
    if host.fingerprint == actual {
        Ok(())
    } else {
        Err(
            "A identidade deste host de internet mudou desde a última conexão. \
             A conexão foi bloqueada por segurança."
                .into(),
        )
    }
}

fn connect_error_text(error: ViewerError) -> String {
    let detail = error.to_string();
    if detail.contains("fingerprint does not match") {
        "A identidade deste host mudou desde a última conexão. \
         A conexão foi bloqueada por segurança. Se o host foi reinstalado, \
         remova manualmente o registro conhecido antes de tentar novamente."
            .into()
    } else {
        format!("falha ao conectar: {detail}")
    }
}

/// Re-encodes each new decoded frame as JPEG and pushes it to the webview.
/// Runs on a dedicated thread: the encode is CPU work and `Channel::send` is
/// synchronous.
fn spawn_frame_pump(
    frame_slot: FrameSlot,
    on_frame: Channel<InvokeResponseBody>,
    stop: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("controlis-frame-pump".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let frame = frame_slot.lock().ok().and_then(|mut slot| slot.take());
                match frame {
                    Some(frame) => match codec::encode_rgba_to_jpeg(&frame) {
                        Ok(jpeg) => {
                            if on_frame.send(InvokeResponseBody::Raw(jpeg)).is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!("frame jpeg encode failed: {e}"),
                    },
                    None => std::thread::sleep(FRAME_POLL),
                }
            }
        })
        .expect("spawn frame pump thread");
}

/// Owns the `ViewerHandle`: forwards status events to the webview and stops
/// the frame pump when the session ends.
async fn pump_events(
    mut handle: session_viewer::ViewerHandle,
    on_event: Channel<ViewerUiEvent>,
    stop_frames: Arc<AtomicBool>,
) {
    while let Some(event) = handle.next_event().await {
        let ui_event = match event {
            // Frames flow through the binary channel; nothing to forward here.
            ViewerEvent::FrameReady => continue,
            ViewerEvent::Resized {
                width_px,
                height_px,
            } => ViewerUiEvent::Resized {
                width_px,
                height_px,
            },
            ViewerEvent::MonitorsUpdated(monitors) => ViewerUiEvent::MonitorsUpdated {
                monitors: monitors.iter().map(MonitorUi::from).collect(),
            },
            ViewerEvent::Disconnected(reason) => {
                stop_frames.store(true, Ordering::Relaxed);
                let _ = on_event.send(ViewerUiEvent::Disconnected { reason });
                break;
            }
            ViewerEvent::Error(message) => ViewerUiEvent::Error { message },
        };
        if on_event.send(ui_event).is_err() {
            break;
        }
    }
    stop_frames.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub fn viewer_input(slot: State<'_, ViewerSlot>, event: UiInputEvent) -> Result<(), String> {
    let slot = slot.0.lock().unwrap();
    let Some(viewer) = slot.as_ref() else {
        return Ok(()); // Session already gone; stale events are harmless.
    };
    if let Some(message) = to_control_message(event) {
        let _ = viewer.input.send(message);
    }
    Ok(())
}

#[tauri::command]
pub fn viewer_select_monitor(slot: State<'_, ViewerSlot>, monitor_id: u32) -> Result<(), String> {
    let slot = slot.0.lock().unwrap();
    if let Some(viewer) = slot.as_ref() {
        let _ = viewer
            .input
            .send(ControlMessage::SelectMonitor { monitor_id });
    }
    Ok(())
}

#[tauri::command]
pub fn viewer_disconnect(slot: State<'_, ViewerSlot>) -> Result<(), String> {
    if let Some(viewer) = slot.0.lock().unwrap().take() {
        let _ = viewer.input.send(ControlMessage::Disconnect {
            reason: "viewer closed".into(),
        });
        // Drop stops the frame pump.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    #[test]
    fn resolve_target_uses_embedded_lan_address_for_peer_key() {
        let code = security::ConnectCode::generate(SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 0, 10),
            21118,
        ));

        let target = resolve_target(code.as_str(), None).unwrap();

        let ResolvedTarget::Lan(target) = target else {
            panic!("expected LAN target");
        };
        assert_eq!(target.addr, SocketAddr::from(([192, 168, 0, 10], 21118)));
        assert_eq!(target.canonical_code, code.as_str());
        assert_eq!(target.peer_key, "lan:192.168.0.10:21118");
        assert_eq!(target.nickname, "LAN 192.168.0.10:21118");
    }

    #[test]
    fn resolve_target_uses_manual_address_for_peer_key() {
        let code = security::ConnectCode::generate(SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 0, 10),
            21118,
        ));

        let target = resolve_target(code.as_str(), Some("10.0.0.5:30000")).unwrap();

        let ResolvedTarget::Lan(target) = target else {
            panic!("expected LAN target");
        };
        assert_eq!(target.addr, SocketAddr::from(([10, 0, 0, 5], 30000)));
        assert_eq!(target.canonical_code, code.as_str());
        assert_eq!(target.peer_key, "lan:10.0.0.5:30000");
        assert_eq!(target.nickname, "LAN 10.0.0.5:30000");
    }

    #[test]
    fn resolve_target_rejects_bad_manual_address() {
        let code = security::ConnectCode::generate(SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 0, 10),
            21118,
        ));

        assert!(resolve_target(code.as_str(), Some("10.0.0.5")).is_err());
    }

    #[test]
    fn resolve_target_accepts_rendezvous_code_as_internet_target() {
        let code = security::ConnectCode::generate_rendezvous(123).unwrap();

        let target = resolve_target(code.as_str(), None).unwrap();

        assert_eq!(
            target,
            ResolvedTarget::Internet {
                session_id: SessionId::new(123).unwrap(),
                canonical_code: code.as_str().to_string()
            }
        );
    }

    #[test]
    fn resolve_target_uses_manual_address_for_rendezvous_code_as_lan_fallback() {
        let code = security::ConnectCode::generate_rendezvous(123).unwrap();

        let target = resolve_target(code.as_str(), Some("10.0.0.5:30000")).unwrap();

        let ResolvedTarget::Lan(target) = target else {
            panic!("expected LAN target");
        };
        assert_eq!(target.addr, SocketAddr::from(([10, 0, 0, 5], 30000)));
        assert_eq!(target.canonical_code, code.as_str());
        assert_eq!(target.peer_key, "lan:10.0.0.5:30000");
    }

    #[test]
    fn expected_fingerprint_reads_known_host_pin() {
        let store = Store::in_memory().unwrap();
        let fingerprint = transport::HostIdentity::generate()
            .unwrap()
            .fingerprint()
            .to_string();
        store
            .remember_host(&KnownHost {
                peer_key: "lan:192.168.0.10:21118".into(),
                fingerprint: fingerprint.clone(),
                nickname: "LAN 192.168.0.10:21118".into(),
            })
            .unwrap();

        let expected = expected_fingerprint(&store, "lan:192.168.0.10:21118").unwrap();

        assert_eq!(expected.unwrap().to_string(), fingerprint);
    }

    #[test]
    fn expected_fingerprint_rejects_corrupt_pin() {
        let store = Store::in_memory().unwrap();
        store
            .remember_host(&KnownHost {
                peer_key: "lan:192.168.0.10:21118".into(),
                fingerprint: "not-a-fingerprint".into(),
                nickname: "LAN 192.168.0.10:21118".into(),
            })
            .unwrap();

        assert!(expected_fingerprint(&store, "lan:192.168.0.10:21118").is_err());
    }

    #[test]
    fn expected_iroh_identity_blocks_changed_endpoint_id() {
        let store = Store::in_memory().unwrap();
        store
            .remember_host(&KnownHost {
                peer_key: "rendezvous:123".into(),
                fingerprint: "iroh:old".into(),
                nickname: "Internet 123".into(),
            })
            .unwrap();

        assert!(expected_iroh_identity(&store, "rendezvous:123", "iroh:new").is_err());
        assert!(expected_iroh_identity(&store, "rendezvous:123", "iroh:old").is_ok());
    }
}
