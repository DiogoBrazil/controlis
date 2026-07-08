//! Viewer-mode commands: connect with an access code, stream frames to the
//! webview canvas and forward input back to the host.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::{ControlMessage, MonitorInfo};
use serde::Serialize;
use session_viewer::{FrameSlot, ViewerEvent};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use tokio::sync::mpsc;

use crate::input_map::{to_control_message, UiInputEvent};

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
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ViewerUiEvent {
    Resized { width_px: u32, height_px: u32 },
    MonitorsUpdated { monitors: Vec<MonitorUi> },
    Disconnected { reason: String },
    Error { message: String },
}

#[tauri::command]
pub async fn connect_viewer(
    slot: State<'_, ViewerSlot>,
    code: String,
    addr_override: Option<String>,
    on_event: Channel<ViewerUiEvent>,
    on_frame: Channel<InvokeResponseBody>,
) -> Result<ViewerInfo, String> {
    drop(slot.0.lock().unwrap().take());

    let (addr, canonical_code) = resolve_target(&code, addr_override.as_deref())?;
    let handle = session_viewer::connect(addr, canonical_code, None)
        .await
        .map_err(|e| format!("falha ao conectar: {e}"))?;

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

/// Turns the typed access code (plus optional manual address) into the
/// connection target and the canonical code to authenticate with.
fn resolve_target(code: &str, addr_override: Option<&str>) -> Result<(SocketAddr, String), String> {
    let code = security::ConnectCode::parse(code.trim())
        .map_err(|e| format!("Código inválido ({e}). Confira com quem está no computador host."))?;
    let addr = match addr_override.map(str::trim).filter(|s| !s.is_empty()) {
        None => code.addr().into(),
        Some(manual) => manual
            .parse::<SocketAddr>()
            .map_err(|_| "Endereço manual inválido. Use IP:porta, ex. 192.168.0.10:21118".to_string())?,
    };
    Ok((addr, code.as_str().to_string()))
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
            ViewerEvent::Resized { width_px, height_px } => {
                ViewerUiEvent::Resized { width_px, height_px }
            }
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
        let _ = viewer.input.send(ControlMessage::SelectMonitor { monitor_id });
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
