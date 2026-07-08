use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codec::VideoEncoder;
use input::{coords, InputInjector};
use protocol::{
    negotiate_codec, AuthOutcome, ControlMessage, KeyCode, MonitorInfo, MouseButton,
    PointerAction, VideoCodec, PROTOCOL_VERSION,
};
use security::{BruteForceGuard, ConnectCode, GuardDecision};
use tokio::sync::mpsc;
use transport::{Connection, ControlChannel};

use crate::event::{ConnectionRequest, HostCommand, HostEvent};
use crate::{CapturerFactory, HostConfig, InjectorFactory};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Input events forwarded from the control loop to the injection thread.
enum InputAction {
    Move(i32, i32),
    Button(MouseButton, PointerAction),
    Wheel(f32, f32),
    Key(KeyCode, PointerAction),
    Text(String),
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_session(
    config: &HostConfig,
    connection: Connection,
    code: &ConnectCode,
    guard: &mut BruteForceGuard,
    capturer_factory: &CapturerFactory,
    injector_factory: &InjectorFactory,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
    command_rx: &mut mpsc::UnboundedReceiver<HostCommand>,
) {
    let peer = connection.remote_address();
    let mut control = match connection.accept_control().await {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(HostEvent::Error(format!("no control stream from {peer}: {e}")));
            return;
        }
    };

    let viewer_codecs =
        match authenticate(config, &connection, &mut control, code, guard, event_tx, command_rx)
            .await
        {
            AuthResult::Accepted { viewer_codecs } => viewer_codecs,
            AuthResult::Ended => {
                connection.close("authentication failed");
                return;
            }
        };

    stream_and_control(
        config,
        connection,
        control,
        &viewer_codecs,
        capturer_factory,
        injector_factory,
        event_tx,
        command_rx,
    )
    .await;
}

enum AuthResult {
    Accepted { viewer_codecs: Vec<VideoCodec> },
    Ended,
}

#[allow(clippy::too_many_arguments)]
async fn authenticate(
    _config: &HostConfig,
    connection: &Connection,
    control: &mut ControlChannel,
    code: &ConnectCode,
    guard: &mut BruteForceGuard,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
    command_rx: &mut mpsc::UnboundedReceiver<HostCommand>,
) -> AuthResult {
    let peer = connection.remote_address();
    let fingerprint = connection.peer_fingerprint().map(|f| f.to_string());

    // Expect Hello, then AuthRequest, each within the handshake timeout.
    let hello = match recv_timeout(control).await {
        Some(ControlMessage::Hello(h)) => h,
        _ => return reject(control, event_tx, peer, "handshake: expected Hello").await,
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = control
            .send(&ControlMessage::Error {
                code: 1,
                message: format!(
                    "protocol version {} unsupported (host speaks {})",
                    hello.protocol_version, PROTOCOL_VERSION
                ),
            })
            .await;
        let _ = event_tx.send(HostEvent::Log(format!(
            "rejected {peer}: protocol version {}",
            hello.protocol_version
        )));
        return AuthResult::Ended;
    }

    let session_code = match recv_timeout(control).await {
        Some(ControlMessage::AuthRequest { session_code }) => session_code,
        _ => return reject(control, event_tx, peer, "handshake: expected AuthRequest").await,
    };

    if let GuardDecision::Blocked { retry_after } = guard.check(peer.ip(), Instant::now()) {
        let _ = control
            .send(&ControlMessage::AuthResponse(AuthOutcome::Rejected {
                reason: format!("too many attempts; retry in {}s", retry_after.as_secs()),
            }))
            .await;
        let _ = event_tx.send(HostEvent::Log(format!("blocked {peer}: brute-force backoff")));
        return AuthResult::Ended;
    }

    if !code.verify(&session_code) {
        guard.record_failure(peer.ip(), Instant::now());
        let _ = control
            .send(&ControlMessage::AuthResponse(AuthOutcome::Rejected {
                reason: "invalid session code".into(),
            }))
            .await;
        let _ = event_tx.send(HostEvent::Log(format!("rejected {peer}: wrong code")));
        return AuthResult::Ended;
    }

    if _config.require_manual_approval
        && !await_approval(control, event_tx, command_rx, peer, fingerprint).await
    {
        return AuthResult::Ended;
    }

    guard.record_success(peer.ip());
    let session_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if control
        .send(&ControlMessage::AuthResponse(AuthOutcome::Accepted { session_id }))
        .await
        .is_err()
    {
        return AuthResult::Ended;
    }
    let _ = event_tx.send(HostEvent::Log(format!("accepted {peer}")));
    AuthResult::Accepted {
        viewer_codecs: hello.supported_codecs,
    }
}

async fn await_approval(
    control: &mut ControlChannel,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
    command_rx: &mut mpsc::UnboundedReceiver<HostCommand>,
    peer: std::net::SocketAddr,
    fingerprint: Option<String>,
) -> bool {
    let _ = control
        .send(&ControlMessage::AuthResponse(AuthOutcome::PendingApproval))
        .await;
    let _ = event_tx.send(HostEvent::ApprovalRequested(ConnectionRequest {
        peer_addr: peer.to_string(),
        fingerprint,
    }));

    match command_rx.recv().await {
        Some(HostCommand::Approve) => true,
        _ => {
            let _ = control
                .send(&ControlMessage::AuthResponse(AuthOutcome::Rejected {
                    reason: "host declined the connection".into(),
                }))
                .await;
            let _ = event_tx.send(HostEvent::Log(format!("declined {peer}")));
            false
        }
    }
}

async fn reject(
    control: &mut ControlChannel,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
    peer: std::net::SocketAddr,
    reason: &str,
) -> AuthResult {
    let _ = control
        .send(&ControlMessage::AuthResponse(AuthOutcome::Rejected {
            reason: reason.into(),
        }))
        .await;
    let _ = event_tx.send(HostEvent::Log(format!("rejected {peer}: {reason}")));
    AuthResult::Ended
}

async fn recv_timeout(control: &mut ControlChannel) -> Option<ControlMessage> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, control.recv())
        .await
        .ok()?
        .ok()
}

#[allow(clippy::too_many_arguments)]
async fn stream_and_control(
    config: &HostConfig,
    connection: Connection,
    mut control: ControlChannel,
    viewer_codecs: &[VideoCodec],
    capturer_factory: &CapturerFactory,
    injector_factory: &InjectorFactory,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
    command_rx: &mut mpsc::UnboundedReceiver<HostCommand>,
) {
    let peer = connection.remote_address();

    let codec = negotiate_codec(config.codec, codec::supported_codecs(), viewer_codecs);
    let encoder = match VideoEncoder::new(codec, config.target_fps) {
        Ok(e) => e,
        Err(e) => {
            let _ = event_tx.send(HostEvent::Error(format!("codec unavailable: {e}")));
            connection.close("codec unavailable");
            return;
        }
    };
    let _ = event_tx.send(HostEvent::Log(format!("negotiated codec: {codec:?}")));

    let capturer = match capturer_factory() {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(HostEvent::Error(format!("capture unavailable: {e}")));
            connection.close("capture unavailable");
            return;
        }
    };
    let monitors = match capturer.monitors() {
        Ok(m) if !m.is_empty() => m,
        _ => {
            let _ = event_tx.send(HostEvent::Error("no monitors to capture".into()));
            connection.close("no monitors");
            return;
        }
    };
    let active = monitors
        .iter()
        .find(|m| m.is_primary)
        .cloned()
        .unwrap_or_else(|| monitors[0].clone());

    let _ = control
        .send(&ControlMessage::MonitorList { monitors: monitors.clone() })
        .await;
    let _ = event_tx.send(HostEvent::SessionStarted { peer_addr: peer.to_string() });

    let stop = Arc::new(AtomicBool::new(false));
    let active_id = Arc::new(AtomicU32::new(active.id));
    // Capacity 1 gives the capture loop backpressure: when encoding or the
    // network cannot keep up, ticks are skipped *before* encoding instead of
    // queueing ever-older frames (which would grow latency without bound and,
    // for H.264, cannot be dropped after encoding without breaking the
    // reference chain).
    let (media_tx, mut media_rx) = mpsc::channel(1);
    let (input_tx, input_rx) = crossbeam_channel::unbounded::<InputAction>();

    let capture_handle =
        spawn_capture(config, capturer, encoder, active_id.clone(), stop.clone(), media_tx);
    spawn_injection(injector_factory.clone(), input_rx, event_tx.clone());

    let media_conn = connection.clone();
    let media_events = event_tx.clone();
    let media_task = tokio::spawn(async move {
        // Bandwidth accounting: payload bytes dominate; framing/QUIC overhead is
        // negligible next to the 4 Mbps acceptance target for phase 7.
        let mut bytes: u64 = 0;
        let mut frames: u64 = 0;
        let mut window_start = Instant::now();
        while let Some(message) = media_rx.recv().await {
            let protocol::MediaMessage::ScreenFrame { ref payload, .. } = message;
            bytes += payload.len() as u64;
            frames += 1;
            if media_conn.send_media(&message).await.is_err() {
                break;
            }
            let elapsed = window_start.elapsed();
            if elapsed >= Duration::from_secs(5) {
                let mbps = (bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0;
                let fps = frames as f64 / elapsed.as_secs_f64();
                let line =
                    format!("mídia ({codec:?}): {mbps:.2} Mbps, {fps:.1} fps");
                tracing::info!("{line}");
                let _ = media_events.send(HostEvent::Log(line));
                bytes = 0;
                frames = 0;
                window_start = Instant::now();
            }
        }
    });

    let reason = control_loop(&mut control, command_rx, &input_tx, &monitors, active_id, active).await;

    // Tear down: stop capture, drop input sender so the injector releases keys,
    // close the connection, and wait for the helper tasks.
    stop.store(true, Ordering::Relaxed);
    drop(input_tx);
    connection.close(&reason);
    let _ = capture_handle.join();
    let _ = media_task.await;

    let _ = event_tx.send(HostEvent::SessionEnded { reason });
}

async fn control_loop(
    control: &mut ControlChannel,
    command_rx: &mut mpsc::UnboundedReceiver<HostCommand>,
    input_tx: &crossbeam_channel::Sender<InputAction>,
    monitors: &[MonitorInfo],
    active_id: Arc<AtomicU32>,
    mut active: MonitorInfo,
) -> String {
    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(HostCommand::Stop) | None => return "host ended the session".into(),
                    _ => {}
                }
            }
            message = control.recv() => {
                match message {
                    Ok(ControlMessage::MouseMove { x_norm, y_norm }) => {
                        let (x, y) = coords::normalized_to_pixel(
                            x_norm, y_norm, active.origin_x, active.origin_y,
                            active.width_px, active.height_px,
                        );
                        let _ = input_tx.send(InputAction::Move(x, y));
                    }
                    Ok(ControlMessage::SelectMonitor { monitor_id }) => {
                        if let Some(monitor) = monitors.iter().find(|m| m.id == monitor_id) {
                            active = monitor.clone();
                            active_id.store(monitor_id, Ordering::Relaxed);
                            let _ = control
                                .send(&ControlMessage::Resize {
                                    width_px: monitor.width_px,
                                    height_px: monitor.height_px,
                                })
                                .await;
                        }
                    }
                    Ok(ControlMessage::MouseButton { button, action }) => {
                        let _ = input_tx.send(InputAction::Button(button, action));
                    }
                    Ok(ControlMessage::MouseWheel { delta_x, delta_y }) => {
                        let _ = input_tx.send(InputAction::Wheel(delta_x, delta_y));
                    }
                    Ok(ControlMessage::KeyEvent { key, action }) => {
                        let _ = input_tx.send(InputAction::Key(key, action));
                    }
                    Ok(ControlMessage::Text { text }) => {
                        let _ = input_tx.send(InputAction::Text(text));
                    }
                    Ok(ControlMessage::Ping { nonce }) => {
                        let _ = control.send(&ControlMessage::Pong { nonce }).await;
                    }
                    Ok(ControlMessage::Disconnect { reason }) => {
                        return format!("viewer disconnected: {reason}");
                    }
                    Ok(_) => {}
                    Err(_) => return "connection lost".into(),
                }
            }
        }
    }
}

fn spawn_capture(
    config: &HostConfig,
    mut capturer: Box<dyn ScreenCapturerAlias>,
    mut encoder: VideoEncoder,
    active_id: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    media_tx: mpsc::Sender<protocol::MediaMessage>,
) -> std::thread::JoinHandle<()> {
    let interval = Duration::from_millis((1000 / config.target_fps.max(1)) as u64);
    std::thread::spawn(move || {
        let mut current_id = active_id.load(Ordering::Relaxed);
        let mut frame_id = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            let requested = active_id.load(Ordering::Relaxed);
            if requested != current_id {
                // Monitor changed: the next frame must stand alone (a delta
                // against the old monitor would be wrong even if the two
                // monitors happen to share dimensions).
                encoder.force_keyframe();
                current_id = requested;
            }
            // Only encode when the sender has drained the previous frame;
            // otherwise skip this tick and let the next one capture fresher
            // content. A frame that would sit in a queue is pure latency.
            let permit = match media_tx.try_reserve() {
                Ok(permit) => permit,
                Err(mpsc::error::TrySendError::Full(())) => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(mpsc::error::TrySendError::Closed(())) => break,
            };
            match capturer.capture(current_id) {
                Ok(frame) => match encoder.encode(current_id, frame_id, &frame) {
                    Ok(Some(message)) => {
                        frame_id += 1;
                        permit.send(message);
                    }
                    // The rate controller skipped this frame; nothing to send.
                    Ok(None) => {}
                    Err(_) => break,
                },
                Err(_) => break,
            }
            if let Some(remaining) = interval.checked_sub(started.elapsed()) {
                std::thread::sleep(remaining);
            }
        }
    })
}

fn spawn_injection(
    injector_factory: InjectorFactory,
    input_rx: crossbeam_channel::Receiver<InputAction>,
    event_tx: mpsc::UnboundedSender<HostEvent>,
) {
    std::thread::spawn(move || {
        let mut injector = match injector_factory() {
            Ok(i) => i,
            Err(e) => {
                let _ = event_tx.send(HostEvent::Error(format!("input unavailable: {e}")));
                return;
            }
        };
        while let Ok(action) = input_rx.recv() {
            let result = apply_input(injector.as_mut(), action);
            if let Err(e) = result {
                let _ = event_tx.send(HostEvent::Error(format!("input error: {e}")));
            }
        }
        // Channel closed (session ended): release everything still held.
        let _ = injector.release_all();
    });
}

fn apply_input(injector: &mut dyn InputInjector, action: InputAction) -> Result<(), input::InputError> {
    match action {
        InputAction::Move(x, y) => injector.move_pointer(x, y),
        InputAction::Button(button, act) => injector.mouse_button(button, act),
        InputAction::Wheel(dx, dy) => injector.mouse_wheel(dx, dy),
        InputAction::Key(key, act) => injector.key(key, act),
        InputAction::Text(text) => injector.text(&text),
    }
}

// Local alias so the capture thread signature does not need to name the trait
// object type with its `Send` bound inline.
use capture::ScreenCapturer as ScreenCapturerAlias;
