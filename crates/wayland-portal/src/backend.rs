use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use capture::{CaptureError, ScreenCapturer};
use codec::RgbaFrame;
use input::{InputError, InputInjector};
use protocol::{KeyCode, MonitorInfo, MouseButton, PointerAction};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::PortalError;

mod capture_thread;
mod eis_input;
mod keysym;

use capture_thread::{spawn_capture, CaptureStop};
use eis_input::{CmdSender, EisInput, Msg};

/// A running combined portal session: capture frames plus input injection.
///
/// One async task owns the ashpd session for its lifetime (the EIS connection and
/// PipeWire stream are only valid while it lives). A PipeWire thread publishes
/// frames; an ei client thread applies input.
#[derive(Debug)]
pub struct PortalHandle {
    monitor: MonitorInfo,
    frame: Arc<Mutex<Option<RgbaFrame>>>,
    input: CmdSender,
    eis: Option<EisInput>,
    capture_stop: Option<CaptureStop>,
    shutdown: Option<oneshot::Sender<()>>,
}

/// Backend-neutral input commands (translated to EIS events by the ei thread).
#[derive(Debug)]
pub(crate) enum InputCmd {
    Motion { x: f64, y: f64 },
    Button { code: i32, pressed: bool },
    Axis { dx: f64, dy: f64 },
    Key { keysym: i32, pressed: bool },
}

struct PortalInit {
    monitor: MonitorInfo,
    node_id: u32,
    capture_fd: OwnedFd,
    eis_fd: OwnedFd,
}

impl PortalHandle {
    /// Creates the portal session (prompting the user for consent) and starts
    /// capture and input. Must be awaited on the provided runtime.
    pub async fn new(rt: Handle) -> Result<Self, PortalError> {
        let (init_tx, init_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        rt.spawn(portal_task(init_tx, shutdown_rx));

        let init = init_rx
            .await
            .map_err(|_| PortalError::Portal("portal task exited before init".into()))??;

        let frame = Arc::new(Mutex::new(None));
        let capture_stop = spawn_capture(init.capture_fd, init.node_id, frame.clone())?;
        let eis = eis_input::spawn(init.eis_fd)?;
        let input = eis.sender();

        Ok(Self {
            monitor: init.monitor,
            frame,
            input,
            eis: Some(eis),
            capture_stop: Some(capture_stop),
            shutdown: Some(shutdown_tx),
        })
    }

    /// A capturer view sharing this session's frame stream.
    pub fn capturer(&self) -> PortalCapturer {
        PortalCapturer {
            frame: self.frame.clone(),
            monitor: self.monitor.clone(),
        }
    }

    /// An injector view sharing this session's input channel.
    pub fn injector(&self) -> PortalInjector {
        PortalInjector {
            input: self.input.clone(),
            held_keys: Vec::new(),
            held_buttons: Vec::new(),
        }
    }

    pub fn monitor(&self) -> &MonitorInfo {
        &self.monitor
    }
}

impl Drop for PortalHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.capture_stop.take() {
            stop.stop();
        }
        if let Some(eis) = self.eis.take() {
            eis.shutdown();
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// Owns the ashpd session for its lifetime. Publishes init data, then keeps the
/// session alive until shutdown is signalled (on [`PortalHandle`] drop).
async fn portal_task(
    init_tx: oneshot::Sender<Result<PortalInit, PortalError>>,
    shutdown_rx: oneshot::Receiver<()>,
) {
    match setup_session().await {
        Ok((_remote, session, init)) => {
            if init_tx.send(Ok(init)).is_err() {
                return;
            }
            let _ = shutdown_rx.await;
            let _ = session.close().await;
        }
        Err(e) => {
            let _ = init_tx.send(Err(e));
        }
    }
}

async fn setup_session(
) -> Result<(RemoteDesktop, Session<RemoteDesktop>, PortalInit), PortalError> {
    let map_err = |e: ashpd::Error| PortalError::Portal(e.to_string());

    let remote = RemoteDesktop::new().await.map_err(map_err)?;
    let screencast = Screencast::new().await.map_err(map_err)?;
    let session = remote.create_session(Default::default()).await.map_err(map_err)?;

    remote
        .select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(map_err)?;

    screencast
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(map_err)?;

    let response = remote
        .start(&session, None, Default::default())
        .await
        .map_err(map_err)?
        .response()
        .map_err(map_err)?;

    tracing::info!(
        "portal granted devices: {:?}, streams: {}",
        response.devices(),
        response.streams().len()
    );

    let stream = response.streams().first().ok_or(PortalError::NoStream)?;
    let node_id = stream.pipe_wire_node_id();
    let (width, height) = stream.size().ok_or(PortalError::NoStream)?;

    let capture_fd = screencast
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(map_err)?;

    // Input on GNOME 45+ goes through libei/EIS, not the portal Notify* methods.
    let eis_fd = remote
        .connect_to_eis(&session, Default::default())
        .await
        .map_err(map_err)?;

    let monitor = MonitorInfo {
        id: node_id,
        name: "Wayland (portal)".into(),
        origin_x: 0,
        origin_y: 0,
        width_px: width as u32,
        height_px: height as u32,
        is_primary: true,
    };

    Ok((remote, session, PortalInit { monitor, node_id, capture_fd, eis_fd }))
}

/// Screen capturer backed by the portal's PipeWire stream.
#[derive(Debug)]
pub struct PortalCapturer {
    frame: Arc<Mutex<Option<RgbaFrame>>>,
    monitor: MonitorInfo,
}

impl ScreenCapturer for PortalCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(vec![self.monitor.clone()])
    }

    fn capture(&mut self, _monitor_id: u32) -> Result<RgbaFrame, CaptureError> {
        // The PipeWire thread publishes frames asynchronously; return the latest,
        // waiting briefly for the first one after negotiation.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(frame) = self.frame.lock().ok().and_then(|f| f.clone()) {
                return Ok(frame);
            }
            if Instant::now() >= deadline {
                return Err(CaptureError::Backend("no frame from portal yet".into()));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

/// Input injector that forwards to the session's ei (EIS) client thread.
#[derive(Debug)]
pub struct PortalInjector {
    input: CmdSender,
    held_keys: Vec<i32>,
    held_buttons: Vec<i32>,
}

impl PortalInjector {
    fn send(&self, cmd: InputCmd) -> Result<(), InputError> {
        self.input
            .send(Msg::Input(cmd))
            .map_err(|_| InputError::Inject("portal input channel closed".into()))
    }
}

impl InputInjector for PortalInjector {
    fn move_pointer(&mut self, x: i32, y: i32) -> Result<(), InputError> {
        self.send(InputCmd::Motion { x: x as f64, y: y as f64 })
    }

    fn mouse_button(&mut self, button: MouseButton, action: PointerAction) -> Result<(), InputError> {
        let code = evdev_button(button);
        let pressed = matches!(action, PointerAction::Press);
        match action {
            PointerAction::Press => self.held_buttons.push(code),
            PointerAction::Release => self.held_buttons.retain(|b| *b != code),
        }
        self.send(InputCmd::Button { code, pressed })
    }

    fn mouse_wheel(&mut self, delta_x: f32, delta_y: f32) -> Result<(), InputError> {
        // ei scroll is positive-down/right; wire uses positive-up for y.
        self.send(InputCmd::Axis {
            dx: delta_x as f64,
            dy: -delta_y as f64,
        })
    }

    fn key(&mut self, key: KeyCode, action: PointerAction) -> Result<(), InputError> {
        let Some(keysym) = keysym::to_keysym(key) else {
            return Ok(());
        };
        let pressed = matches!(action, PointerAction::Press);
        match action {
            PointerAction::Press => self.held_keys.push(keysym),
            PointerAction::Release => self.held_keys.retain(|k| *k != keysym),
        }
        self.send(InputCmd::Key { keysym, pressed })
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        let keys: Vec<i32> = self.held_keys.drain(..).collect();
        let buttons: Vec<i32> = self.held_buttons.drain(..).collect();
        for keysym in keys {
            let _ = self.send(InputCmd::Key { keysym, pressed: false });
        }
        for code in buttons {
            let _ = self.send(InputCmd::Button { code, pressed: false });
        }
        Ok(())
    }
}

fn evdev_button(button: MouseButton) -> i32 {
    // Linux input-event-codes: BTN_LEFT/RIGHT/MIDDLE.
    match button {
        MouseButton::Left => 0x110,
        MouseButton::Right => 0x111,
        MouseButton::Middle => 0x112,
    }
}
