//! Host-mode commands: start/stop the host service and forward its events to
//! the webview through a channel.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use session_host::{HostCommand, HostConfig, HostEvent};
use storage::{ConnectionLog, Store};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::{mpsc, oneshot};

use crate::backend::{build_backend, detect_lan_ip};
use crate::AppEnv;

/// The running host service, owned by the UI session that started it.
pub struct RunningHost {
    commands: mpsc::UnboundedSender<HostCommand>,
    shutdown: Option<oneshot::Sender<()>>,
    /// Keeps a Wayland portal session alive for the host's lifetime, if used.
    /// Never read: it exists only so its Drop runs when the host stops.
    _keepalive: Option<Box<dyn std::any::Any + Send>>,
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // `keepalive` (portal session) drops afterwards; PortalHandle::drop
        // waits for the portal to close cleanly.
    }
}

#[derive(Default)]
pub struct HostSlot(pub Arc<Mutex<Option<RunningHost>>>);

/// Static facts about the started host, returned by `start_host`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub port: u16,
    pub fingerprint: String,
    pub backend_label: String,
}

/// Events streamed to the webview while the host runs.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostUiEvent {
    CodeReady { code: String },
    ApprovalRequested { peer_addr: String, fingerprint: Option<String> },
    SessionStarted { peer_addr: String },
    SessionEnded { reason: String },
    Log { line: String },
    Error { message: String },
    Stopped,
}

#[tauri::command]
pub async fn start_host(
    env: State<'_, AppEnv>,
    slot: State<'_, HostSlot>,
    on_event: Channel<HostUiEvent>,
) -> Result<HostInfo, String> {
    // A previous host (e.g. before a webview reload) is replaced.
    drop(slot.0.lock().unwrap().take());

    let advertised_ip = env.config.advertised_ip.or_else(detect_lan_ip);
    let host_config = HostConfig {
        bind_addr: SocketAddr::from(([0, 0, 0, 0], env.config.host_port)),
        require_manual_approval: env.config.require_manual_approval,
        codec: codec::preferred_codec(),
        advertised_ip,
        ..HostConfig::default()
    };

    // The Wayland backend blocks on the portal consent dialog; never on the
    // async executor.
    let rt = tokio::runtime::Handle::current();
    let backend = tokio::task::spawn_blocking(move || build_backend(&rt))
        .await
        .map_err(|e| e.to_string())?;

    let controller =
        session_host::start(host_config, env.identity.clone(), backend.capturer, backend.injector)
            .map_err(|e| format!("falha ao iniciar o host: {e}"))?;

    if let Some(line) = backend.fallback {
        let _ = on_event.send(HostUiEvent::Log { line });
    }
    if advertised_ip.is_none() {
        let _ = on_event.send(HostUiEvent::Log {
            line: "IP da rede local não detectado: o código só funciona nesta máquina. \
                   Defina advertised_ip no config.toml ou use o modo avançado no viewer."
                .into(),
        });
    }

    let info = HostInfo {
        port: controller.local_addr().port(),
        fingerprint: controller.fingerprint().to_string(),
        backend_label: backend.label,
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    *slot.0.lock().unwrap() = Some(RunningHost {
        commands: controller.command_sender(),
        shutdown: Some(shutdown_tx),
        _keepalive: backend.keepalive,
    });

    tokio::spawn(pump_events(controller, on_event, shutdown_rx, env.db_path.clone()));
    Ok(info)
}

/// Owns the controller for the host's lifetime: forwards events to the webview
/// and records the audit log. Dropping the controller (on shutdown or when the
/// webview goes away) ends the host accept loop.
async fn pump_events(
    mut controller: session_host::HostController,
    on_event: Channel<HostUiEvent>,
    mut shutdown_rx: oneshot::Receiver<()>,
    db_path: PathBuf,
) {
    let store = Store::open(&db_path)
        .map_err(|e| tracing::warn!("audit log unavailable: {e}"))
        .ok();

    let mut current_peer: Option<String> = None;
    loop {
        let event = tokio::select! {
            _ = &mut shutdown_rx => break,
            event = controller.next_event() => match event {
                Some(event) => event,
                None => break,
            },
        };
        let ui_event = match event {
            HostEvent::CodeReady(code) => HostUiEvent::CodeReady { code },
            HostEvent::ApprovalRequested(request) => HostUiEvent::ApprovalRequested {
                peer_addr: request.peer_addr,
                fingerprint: request.fingerprint,
            },
            HostEvent::SessionStarted { peer_addr } => {
                audit(&store, &peer_addr, true, "session started");
                current_peer = Some(peer_addr.clone());
                HostUiEvent::SessionStarted { peer_addr }
            }
            HostEvent::SessionEnded { reason } => {
                let peer = current_peer.take().unwrap_or_else(|| "unknown".into());
                audit(&store, &peer, false, &format!("session ended: {reason}"));
                HostUiEvent::SessionEnded { reason }
            }
            HostEvent::Log(line) => {
                if line.starts_with("rejected")
                    || line.starts_with("blocked")
                    || line.starts_with("declined")
                {
                    audit(&store, "unknown", false, &line);
                }
                HostUiEvent::Log { line }
            }
            HostEvent::Error(message) => HostUiEvent::Error { message },
        };
        if on_event.send(ui_event).is_err() {
            // Webview gone (reload/navigation): stop the host.
            break;
        }
    }
    let _ = on_event.send(HostUiEvent::Stopped);
}

fn audit(store: &Option<Store>, peer: &str, accepted: bool, detail: &str) {
    if let Some(store) = store {
        let _ = store.record_connection(&ConnectionLog {
            peer_addr: peer.to_string(),
            peer_fingerprint: None,
            accepted,
            detail: detail.to_string(),
        });
    }
}

#[tauri::command]
pub fn host_command(slot: State<'_, HostSlot>, action: String) -> Result<(), String> {
    let slot = slot.0.lock().unwrap();
    let Some(running) = slot.as_ref() else {
        return Err("host não está ativo".into());
    };
    let command = match action.as_str() {
        "approve" => HostCommand::Approve,
        "reject" => HostCommand::Reject,
        "stop" => HostCommand::Stop,
        other => return Err(format!("comando desconhecido: {other}")),
    };
    let _ = running.commands.send(command);
    Ok(())
}

#[tauri::command]
pub async fn stop_host(slot: State<'_, HostSlot>) -> Result<(), String> {
    let running = slot.0.lock().unwrap().take();
    if let Some(running) = running {
        // PortalHandle::drop blocks briefly waiting for the portal to close;
        // keep that off the async executor.
        let _ = tokio::task::spawn_blocking(move || drop(running)).await;
    }
    Ok(())
}
