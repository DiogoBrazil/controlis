//! Viewer-side session orchestration: connect to a host, authenticate with the
//! session code, then receive/decode frames and send input.
//!
//! Decoded frames are published into a shared latest-frame slot the UI drains at
//! its own repaint rate; small status changes come through an event channel.

mod event;

pub use event::ViewerEvent;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use codec::{RgbaFrame, VideoDecoder};
use protocol::{ControlMessage, Hello, MonitorInfo, Role, PROTOCOL_VERSION};
use tokio::sync::mpsc;
use transport::{
    connect_iroh, connect_viewer, Connection, ControlReceiver, ControlSender, Fingerprint,
    IrohEndpointDescriptor, IrohSecretKey,
};

/// The latest decoded frame, or `None` until one arrives. The UI takes the frame
/// when present and uploads it as a texture.
pub type FrameSlot = Arc<Mutex<Option<RgbaFrame>>>;

/// Errors from establishing a viewer session.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("connection was rejected: {0}")]
    Rejected(String),
    #[error("host speaks an incompatible protocol")]
    IncompatibleProtocol,
    #[error("unexpected message during handshake")]
    UnexpectedMessage,
    #[error(transparent)]
    Transport(#[from] transport::TransportError),
}

/// Handle the UI holds to a running viewer session.
#[derive(Debug)]
pub struct ViewerHandle {
    events: mpsc::UnboundedReceiver<ViewerEvent>,
    input: mpsc::UnboundedSender<ControlMessage>,
    frame: FrameSlot,
    monitors: Vec<MonitorInfo>,
    fingerprint: Option<String>,
}

impl ViewerHandle {
    /// Queues an input (or other control) message to send to the host.
    pub fn send_input(&self, message: ControlMessage) {
        let _ = self.input.send(message);
    }

    /// A clonable input sender, for UIs where the handle itself is owned by an
    /// event-pump task.
    pub fn input_sender(&self) -> mpsc::UnboundedSender<ControlMessage> {
        self.input.clone()
    }

    /// Receives the next status event, or `None` when the session has ended.
    pub async fn next_event(&mut self) -> Option<ViewerEvent> {
        self.events.recv().await
    }

    /// Non-blocking event poll for synchronous UI loops.
    pub fn try_next_event(&mut self) -> Option<ViewerEvent> {
        self.events.try_recv().ok()
    }

    /// The shared latest-frame slot for the UI to render.
    pub fn frame_slot(&self) -> FrameSlot {
        self.frame.clone()
    }

    pub fn monitors(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    pub fn fingerprint(&self) -> Option<&str> {
        self.fingerprint.as_deref()
    }

    /// Requests the host to switch the captured monitor.
    pub fn select_monitor(&self, monitor_id: u32) {
        self.send_input(ControlMessage::SelectMonitor { monitor_id });
    }

    /// Sends a disconnect notice to the host.
    pub fn disconnect(&self) {
        self.send_input(ControlMessage::Disconnect {
            reason: "viewer closed".into(),
        });
    }
}

/// Connects to a host at `addr`, authenticates with `code`, and starts streaming.
///
/// `expected` is the pinned host fingerprint from a previous session, if any.
/// Returns once the session is accepted; blocks (awaiting) through pending approval.
pub async fn connect(
    addr: SocketAddr,
    code: String,
    expected: Option<Fingerprint>,
) -> Result<ViewerHandle, ViewerError> {
    tracing::info!("viewer: conectando ao host {addr} (LAN, pin conhecido: {})", expected.is_some());
    let seen: transport::SeenFingerprint = Arc::new(Mutex::new(None));
    let connection = connect_viewer(addr, expected, seen.clone()).await?;
    let fingerprint = seen
        .lock()
        .ok()
        .and_then(|f| f.as_ref().map(|f| f.to_string()));
    tracing::info!(
        "viewer: transporte QUIC estabelecido com {addr} (fingerprint {})",
        fingerprint.as_deref().unwrap_or("desconhecida")
    );
    connect_over(connection, code, fingerprint).await
}

pub async fn connect_internet(
    secret_key: IrohSecretKey,
    relay_url: &str,
    descriptor: IrohEndpointDescriptor,
    code: String,
) -> Result<ViewerHandle, ViewerError> {
    tracing::info!(
        "viewer: conectando via Iroh ao endpoint {} (relay {relay_url})",
        descriptor.endpoint_id
    );
    let connection = connect_iroh(secret_key, relay_url, &descriptor).await?;
    let fingerprint = connection.peer_iroh_id().map(|id| format!("iroh:{id}"));
    tracing::info!(
        "viewer: transporte Iroh estabelecido (identidade {})",
        fingerprint.as_deref().unwrap_or("desconhecida")
    );
    connect_over(connection, code, fingerprint).await
}

async fn connect_over(
    connection: Connection,
    code: String,
    fingerprint: Option<String>,
) -> Result<ViewerHandle, ViewerError> {
    let mut control = connection.open_control().await?;

    control
        .send(&ControlMessage::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            role: Role::Viewer,
            supported_codecs: codec::supported_codecs().to_vec(),
        }))
        .await?;
    control
        .send(&ControlMessage::AuthRequest { session_code: code })
        .await?;

    // Await the auth outcome, tolerating a PendingApproval notice first.
    loop {
        match control.recv().await? {
            ControlMessage::AuthResponse(protocol::AuthOutcome::Accepted { .. }) => {
                tracing::info!("viewer: sessão aceita pelo host");
                break;
            }
            ControlMessage::AuthResponse(protocol::AuthOutcome::PendingApproval) => {
                tracing::info!("viewer: aguardando aprovação manual do host");
                continue;
            }
            ControlMessage::AuthResponse(protocol::AuthOutcome::Rejected { reason }) => {
                tracing::warn!("viewer: conexão rejeitada pelo host: {reason}");
                return Err(ViewerError::Rejected(reason));
            }
            ControlMessage::Error { .. } => return Err(ViewerError::IncompatibleProtocol),
            _ => return Err(ViewerError::UnexpectedMessage),
        }
    }

    // Host sends the monitor list right after acceptance.
    let monitors = match control.recv().await? {
        ControlMessage::MonitorList { monitors } => monitors,
        _ => Vec::new(),
    };
    tracing::info!("viewer: host anunciou {} monitor(es)", monitors.len());

    let (sender, receiver) = control.split();

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let frame: FrameSlot = Arc::new(Mutex::new(None));

    spawn_input_task(sender, input_rx, event_tx.clone());
    spawn_control_task(receiver, event_tx.clone());
    spawn_media_task(connection, frame.clone(), event_tx);

    Ok(ViewerHandle {
        events: event_rx,
        input: input_tx,
        frame,
        monitors,
        fingerprint,
    })
}

fn spawn_input_task(
    mut sender: ControlSender,
    mut input_rx: mpsc::UnboundedReceiver<ControlMessage>,
    event_tx: mpsc::UnboundedSender<ViewerEvent>,
) {
    tokio::spawn(async move {
        while let Some(message) = input_rx.recv().await {
            if sender.send(&message).await.is_err() {
                let _ = event_tx.send(ViewerEvent::Disconnected("send failed".into()));
                break;
            }
        }
    });
}

fn spawn_control_task(mut receiver: ControlReceiver, event_tx: mpsc::UnboundedSender<ViewerEvent>) {
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(ControlMessage::Resize {
                    width_px,
                    height_px,
                }) => {
                    let _ = event_tx.send(ViewerEvent::Resized {
                        width_px,
                        height_px,
                    });
                }
                Ok(ControlMessage::MonitorList { monitors }) => {
                    let _ = event_tx.send(ViewerEvent::MonitorsUpdated(monitors));
                }
                Ok(ControlMessage::Disconnect { reason }) => {
                    tracing::info!("viewer: host encerrou a sessão: {reason}");
                    let _ = event_tx.send(ViewerEvent::Disconnected(reason));
                    break;
                }
                Ok(ControlMessage::Error { message, .. }) => {
                    let _ = event_tx.send(ViewerEvent::Error(message));
                }
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!("viewer: conexão de controle perdida");
                    let _ = event_tx.send(ViewerEvent::Disconnected("connection lost".into()));
                    break;
                }
            }
        }
    });
}

fn spawn_media_task(
    connection: Connection,
    frame: FrameSlot,
    event_tx: mpsc::UnboundedSender<ViewerEvent>,
) {
    tokio::spawn(async move {
        let mut decoder = VideoDecoder::new();
        let mut first_frame = true;
        loop {
            match connection.recv_media().await {
                Ok(message) => match decoder.decode(&message) {
                    // `None` means the message carried no picture yet (e.g.
                    // H.264 parameter sets before the first keyframe).
                    Ok(Some(current)) => {
                        if first_frame {
                            first_frame = false;
                            tracing::info!(
                                "viewer: primeiro frame decodificado ({}x{})",
                                current.width,
                                current.height
                            );
                        }
                        if let Ok(mut slot) = frame.lock() {
                            *slot = Some(current.clone());
                        }
                        let _ = event_tx.send(ViewerEvent::FrameReady);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = event_tx.send(ViewerEvent::Error(format!("decode failed: {e}")));
                    }
                },
                Err(_) => {
                    tracing::info!("viewer: stream de mídia encerrado");
                    let _ = event_tx.send(ViewerEvent::Disconnected("stream ended".into()));
                    break;
                }
            }
        }
    });
}
