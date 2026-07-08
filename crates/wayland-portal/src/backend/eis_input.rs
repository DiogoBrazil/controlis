//! libei (EIS) input client.
//!
//! GNOME 45+ no longer honors the portal's `NotifyPointer*`/`NotifyKeyboard*`
//! methods; input is delivered over an EIS connection obtained from
//! `RemoteDesktop.ConnectToEIS`. This runs a small ei client on a dedicated
//! calloop thread that binds pointer and keyboard capabilities and forwards our
//! input commands as emulated events.
//!
//! Keyboard: the server sends an xkb keymap; we build a keysym → evdev-keycode map
//! from it. The viewer already sends explicit modifier keys (Shift/Ctrl/…), so we
//! map each incoming keysym straight to its keycode and rely on those modifiers —
//! AltGr/dead-key composition is a known gap.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::thread::JoinHandle;
use std::time::Instant;

use calloop::generic::Generic;
use reis::ei;
use reis::{Interface, PendingRequestResult};
use xkbcommon::xkb;

use super::InputCmd;
use crate::PortalError;

/// Interfaces we advertise in the ei handshake.
const INTERFACES: &[(&str, u32)] = &[
    ("ei_callback", 1),
    ("ei_connection", 1),
    ("ei_seat", 1),
    ("ei_device", 1),
    ("ei_pingpong", 1),
    ("ei_pointer", 1),
    ("ei_pointer_absolute", 1),
    ("ei_button", 1),
    ("ei_scroll", 1),
    ("ei_keyboard", 1),
];

/// Capabilities we bind (pointer motion/buttons/scroll and keyboard).
const WANTED_CAPS: &[&str] = &[
    "ei_pointer_absolute",
    "ei_button",
    "ei_scroll",
    "ei_keyboard",
];

/// X11/evdev keycode offset: evdev keycode = xkb keycode − 8.
const EVDEV_OFFSET: u32 = 8;

pub(crate) enum Msg {
    Input(InputCmd),
    Shutdown,
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Msg")
    }
}

pub(crate) type CmdSender = calloop::channel::Sender<Msg>;

/// Owns the ei client thread; forwards input commands and shuts it down.
pub(crate) struct EisInput {
    tx: CmdSender,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for EisInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EisInput")
    }
}

impl EisInput {
    pub fn sender(&self) -> CmdSender {
        self.tx.clone()
    }

    pub fn shutdown(mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn spawn(fd: OwnedFd) -> Result<EisInput, PortalError> {
    let (tx, channel) = calloop::channel::channel::<Msg>();
    let join = std::thread::Builder::new()
        .name("controlis-eis".into())
        .spawn(move || {
            if let Err(e) = run(fd, channel) {
                tracing::error!("eis input loop failed: {e}");
            }
        })
        .map_err(|e| PortalError::Portal(e.to_string()))?;
    Ok(EisInput { tx, join: Some(join) })
}

fn run(fd: OwnedFd, channel: calloop::channel::Channel<Msg>) -> Result<(), PortalError> {
    let context = ei::Context::new(UnixStream::from(fd)).map_err(err)?;
    let _handshake = context.handshake();
    context.flush().map_err(err)?;

    let mut event_loop: calloop::EventLoop<State> = calloop::EventLoop::try_new().map_err(err)?;
    let handle = event_loop.handle();

    // The Generic source only tracks fd readiness; reads/writes go through the
    // state's own cloned context (calloop's NoIoDrop wrapper is read-only, and the
    // ei::Context methods take &self anyway).
    let source = Generic::new(context.clone(), calloop::Interest::READ, calloop::Mode::Level);
    handle
        .insert_source(source, |_readiness, _context, state: &mut State| state.on_readable())
        .map_err(err)?;

    handle
        .insert_source(channel, |event, _, state: &mut State| match event {
            calloop::channel::Event::Msg(Msg::Input(cmd)) => state.on_input(cmd),
            calloop::channel::Event::Msg(Msg::Shutdown) | calloop::channel::Event::Closed => {
                state.running = false;
            }
        })
        .map_err(err)?;

    let mut state = State::new(context);
    while state.running {
        event_loop.dispatch(None, &mut state).map_err(err)?;
    }
    Ok(())
}

#[derive(Default)]
struct SeatData {
    capabilities: HashMap<String, u64>,
}

#[derive(Default)]
struct DeviceData {
    interfaces: HashMap<String, reis::Object>,
}

struct PointerDevice {
    device: ei::Device,
    absolute: Option<ei::PointerAbsolute>,
    button: Option<ei::Button>,
    scroll: Option<ei::Scroll>,
    emulating: bool,
}

struct KeyboardDevice {
    device: ei::Device,
    keyboard: ei::Keyboard,
    emulating: bool,
}

struct State {
    context: ei::Context,
    seats: HashMap<ei::Seat, SeatData>,
    devices: HashMap<ei::Device, DeviceData>,
    pointer: Option<PointerDevice>,
    keyboard: Option<KeyboardDevice>,
    /// keysym → evdev keycode, built from the server's xkb keymap.
    keymap: Option<HashMap<u32, u32>>,
    last_serial: u32,
    sequence: u32,
    running: bool,
    start: Instant,
    pending: Vec<InputCmd>,
}

impl State {
    fn new(context: ei::Context) -> Self {
        Self {
            context,
            seats: HashMap::new(),
            devices: HashMap::new(),
            pointer: None,
            keyboard: None,
            keymap: None,
            last_serial: 0,
            sequence: 0,
            running: true,
            start: Instant::now(),
            pending: Vec::new(),
        }
    }

    fn now_micros(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn on_readable(&mut self) -> std::io::Result<calloop::PostAction> {
        if self.context.read().is_err() {
            self.running = false;
            return Ok(calloop::PostAction::Remove);
        }
        while let Some(result) = self.context.pending_event() {
            if let PendingRequestResult::Request(request) = result {
                self.handle(request);
            }
        }
        let _ = self.context.flush();
        Ok(calloop::PostAction::Continue)
    }

    fn handle(&mut self, event: ei::Event) {
        match event {
            ei::Event::Handshake(handshake, ei::handshake::Event::HandshakeVersion { .. }) => {
                handshake.handshake_version(1);
                handshake.name("controlis");
                handshake.context_type(ei::handshake::ContextType::Sender);
                for (interface, version) in INTERFACES {
                    handshake.interface_version(interface, *version);
                }
                handshake.finish();
            }
            ei::Event::Handshake(_, ei::handshake::Event::Connection { serial, .. }) => {
                self.last_serial = serial;
            }
            ei::Event::Connection(_, ei::connection::Event::Seat { seat }) => {
                self.seats.insert(seat, SeatData::default());
            }
            ei::Event::Connection(_, ei::connection::Event::Ping { ping }) => {
                ping.done(0);
            }
            ei::Event::Seat(seat, request) => self.handle_seat(seat, request),
            ei::Event::Device(device, request) => self.handle_device(device, request),
            ei::Event::Keyboard(_, ei::keyboard::Event::Keymap { size, keymap, .. }) => {
                self.load_keymap(keymap, size);
            }
            _ => {}
        }
    }

    fn handle_seat(&mut self, seat: ei::Seat, request: ei::seat::Event) {
        match request {
            ei::seat::Event::Capability { mask, interface } => {
                if let Some(data) = self.seats.get_mut(&seat) {
                    data.capabilities.insert(interface, mask);
                }
            }
            ei::seat::Event::Done => {
                if let Some(data) = self.seats.get(&seat) {
                    let mask = WANTED_CAPS
                        .iter()
                        .filter_map(|name| data.capabilities.get(*name))
                        .fold(0u64, |acc, m| acc | *m);
                    tracing::info!(
                        "ei seat ready; capabilities={:?}, binding mask={mask}",
                        data.capabilities.keys().collect::<Vec<_>>()
                    );
                    if mask != 0 {
                        seat.bind(mask);
                    } else {
                        tracing::warn!("ei seat advertises none of the wanted capabilities");
                    }
                }
            }
            ei::seat::Event::Device { device } => {
                self.devices.insert(device, DeviceData::default());
            }
            _ => {}
        }
    }

    fn handle_device(&mut self, device: ei::Device, request: ei::device::Event) {
        match request {
            ei::device::Event::Interface { object } => {
                if let Some(data) = self.devices.get_mut(&device) {
                    data.interfaces.insert(object.interface().to_owned(), object);
                }
            }
            ei::device::Event::Done => self.finish_device(device),
            ei::device::Event::Resumed { serial } => {
                self.last_serial = serial;
                self.resume(&device);
            }
            ei::device::Event::Paused { serial } => {
                self.last_serial = serial;
                if self.pointer.as_ref().is_some_and(|p| p.device == device) {
                    self.pointer.as_mut().unwrap().emulating = false;
                }
                if self.keyboard.as_ref().is_some_and(|k| k.device == device) {
                    self.keyboard.as_mut().unwrap().emulating = false;
                }
            }
            _ => {}
        }
    }

    fn finish_device(&mut self, device: ei::Device) {
        let Some(data) = self.devices.get(&device) else { return };
        if let Some(keyboard) = downcast::<ei::Keyboard>(data) {
            tracing::info!("ei keyboard device ready");
            self.keyboard = Some(KeyboardDevice { device, keyboard, emulating: false });
            return;
        }
        let absolute = downcast::<ei::PointerAbsolute>(data);
        let button = downcast::<ei::Button>(data);
        let scroll = downcast::<ei::Scroll>(data);
        if absolute.is_some() || button.is_some() || scroll.is_some() {
            tracing::info!(
                "ei pointer device ready (abs={}, button={}, scroll={})",
                absolute.is_some(),
                button.is_some(),
                scroll.is_some()
            );
            self.pointer = Some(PointerDevice { device, absolute, button, scroll, emulating: false });
        }
    }

    fn resume(&mut self, device: &ei::Device) {
        let serial = self.last_serial;
        let sequence = self.sequence;
        let mut started = false;
        if let Some(pointer) = &mut self.pointer {
            if pointer.device == *device && !pointer.emulating {
                pointer.device.start_emulating(serial, sequence);
                pointer.emulating = true;
                started = true;
            }
        }
        if let Some(keyboard) = &mut self.keyboard {
            if keyboard.device == *device && !keyboard.emulating {
                keyboard.device.start_emulating(serial, sequence);
                keyboard.emulating = true;
                started = true;
            }
        }
        if started {
            let kind = if self.keyboard.as_ref().is_some_and(|k| k.device == *device && k.emulating)
            {
                "keyboard"
            } else {
                "pointer"
            };
            tracing::info!(
                "ei {kind} device resumed; emulating ({} events queued)",
                self.pending.len()
            );
            self.sequence += 1;
            self.flush_pending();
        }
    }

    // SAFETY: `new_from_fd` mmaps the keymap fd the compositor sent over the ei
    // connection; the fd is valid and owned here, and `size` is the size it
    // reported for that fd.
    #[allow(unsafe_code)]
    fn load_keymap(&mut self, fd: OwnedFd, size: u32) {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size as usize,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        let keymap = match keymap {
            Ok(Some(keymap)) => keymap,
            _ => {
                tracing::warn!("failed to load ei keyboard keymap; keyboard disabled");
                return;
            }
        };

        let mut reverse: HashMap<u32, u32> = HashMap::new();
        for raw in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
            let keycode = xkb::Keycode::new(raw);
            for level in 0..2 {
                for sym in keymap.key_get_syms_by_level(keycode, 0, level) {
                    reverse.entry(sym.raw()).or_insert(raw.saturating_sub(EVDEV_OFFSET));
                }
            }
        }
        tracing::info!("ei keymap loaded ({} keysyms mapped)", reverse.len());
        self.keymap = Some(reverse);
        self.flush_pending();
    }

    fn flush_pending(&mut self) {
        let queued: Vec<InputCmd> = std::mem::take(&mut self.pending);
        for cmd in queued {
            if self.can_apply(&cmd) {
                self.apply(cmd);
            } else {
                self.pending.push(cmd);
            }
        }
        let _ = self.context.flush();
    }

    fn can_apply(&self, cmd: &InputCmd) -> bool {
        match cmd {
            InputCmd::Key { .. } => {
                self.keymap.is_some() && self.keyboard.as_ref().is_some_and(|k| k.emulating)
            }
            _ => self.pointer.as_ref().is_some_and(|p| p.emulating),
        }
    }

    fn on_input(&mut self, cmd: InputCmd) {
        if self.can_apply(&cmd) {
            self.apply(cmd);
            let _ = self.context.flush();
        } else {
            self.pending.push(cmd);
        }
    }

    fn apply(&mut self, cmd: InputCmd) {
        let serial = self.last_serial;
        let time = self.now_micros();
        match cmd {
            InputCmd::Motion { x, y } => {
                if let Some(pointer) = self.pointer.as_ref() {
                    if let Some(absolute) = &pointer.absolute {
                        absolute.motion_absolute(x as f32, y as f32);
                    }
                    pointer.device.frame(serial, time);
                }
            }
            InputCmd::Button { code, pressed } => {
                if let Some(pointer) = self.pointer.as_ref() {
                    if let Some(button) = &pointer.button {
                        button.button(code as u32, button_state(pressed));
                    }
                    pointer.device.frame(serial, time);
                }
            }
            InputCmd::Axis { dx, dy } => {
                if let Some(pointer) = self.pointer.as_ref() {
                    if let Some(scroll) = &pointer.scroll {
                        scroll.scroll(dx as f32, dy as f32);
                    }
                    pointer.device.frame(serial, time);
                }
            }
            InputCmd::Key { keysym, pressed } => {
                let code = self.keymap.as_ref().and_then(|m| m.get(&(keysym as u32)).copied());
                if let (Some(code), Some(keyboard)) = (code, self.keyboard.as_ref()) {
                    keyboard.keyboard.key(code, key_state(pressed));
                    keyboard.device.frame(serial, time);
                }
            }
        }
    }
}

fn downcast<T: Interface>(data: &DeviceData) -> Option<T> {
    data.interfaces.get(T::NAME)?.clone().downcast()
}

fn button_state(pressed: bool) -> ei::button::ButtonState {
    if pressed {
        ei::button::ButtonState::Press
    } else {
        ei::button::ButtonState::Released
    }
}

fn key_state(pressed: bool) -> ei::keyboard::KeyState {
    if pressed {
        ei::keyboard::KeyState::Press
    } else {
        ei::keyboard::KeyState::Released
    }
}

fn err<E: std::fmt::Display>(e: E) -> PortalError {
    PortalError::Portal(e.to_string())
}
