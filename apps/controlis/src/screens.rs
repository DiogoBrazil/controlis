use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::mpsc;

use std::path::Path;

use egui::{Color32, RichText};
use session_host::{start, ConnectionRequest, HostCommand, HostConfig, HostController, HostEvent};
use session_viewer::{connect, ViewerError, ViewerEvent, ViewerHandle};
use storage::{Config, ConnectionLog, Store};
use tokio::runtime::Handle;
use transport::HostIdentity;

use session_host::{CapturerFactory, InjectorFactory};

use crate::factories::{capturer_factory, injector_factory};
use crate::input_map::{self, InputState};

const MAX_LOG_LINES: usize = 200;

type BackendKeepalive = Option<Box<dyn std::any::Any + Send>>;

/// The capture/input backend picked for this host, surfaced in the UI so a
/// silent fallback (e.g. portal consent denied) is visible to the user.
struct BackendChoice {
    capturer: CapturerFactory,
    injector: InjectorFactory,
    keepalive: BackendKeepalive,
    /// Human-readable backend name shown in the host screen.
    label: String,
    /// Log line explaining a fallback, when one happened.
    fallback: Option<String>,
}

/// Selects the host backend at compile time: Wayland portal when the `wayland`
/// feature is on (falling back to the default backend if consent fails), otherwise
/// the default synthetic/xcap capturer plus the enigo injector.
fn build_backend(handle: &Handle) -> BackendChoice {
    #[cfg(feature = "wayland")]
    let fallback: Option<String> = match crate::factories::wayland_backend(handle) {
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

/// Best-effort LAN IPv4 for the access code; a `advertised_ip` in config.toml
/// takes precedence (machines with VPNs/virtual adapters may need it).
fn detect_lan_ip() -> Option<std::net::Ipv4Addr> {
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

/// Host mode: this machine is controlled by a remote viewer.
pub struct HostScreen {
    controller: HostController,
    code: Option<String>,
    pending: Option<ConnectionRequest>,
    session_active: bool,
    peer: Option<String>,
    log: VecDeque<String>,
    store: Option<Store>,
    /// Which capture/input backend this host is using (shown in the UI).
    backend_label: String,
    /// Keeps a Wayland portal session alive for the host's lifetime, if used.
    _backend_keepalive: Option<Box<dyn std::any::Any + Send>>,
}

impl HostScreen {
    pub fn start(handle: Handle, config: &Config, identity: HostIdentity, db_path: &Path) -> Self {
        let advertised_ip = config.advertised_ip.or_else(detect_lan_ip);
        let host_config = HostConfig {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], config.host_port)),
            require_manual_approval: config.require_manual_approval,
            codec: codec::preferred_codec(),
            advertised_ip,
            ..HostConfig::default()
        };
        let backend = build_backend(&handle);

        // start() spawns tasks and therefore must run inside the runtime context.
        let _guard = handle.enter();
        let controller = start(host_config, identity, backend.capturer, backend.injector)
            .expect("host failed to start");
        let store = match Store::open(db_path) {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!("audit log unavailable: {e}");
                None
            }
        };
        let mut screen = Self {
            controller,
            code: None,
            pending: None,
            session_active: false,
            peer: None,
            log: VecDeque::new(),
            store,
            backend_label: backend.label,
            _backend_keepalive: backend.keepalive,
        };
        if let Some(line) = backend.fallback {
            screen.push_log(line);
        }
        if advertised_ip.is_none() {
            screen.push_log(
                "IP da rede local não detectado: o código só funciona nesta máquina. \
                 Defina advertised_ip no config.toml ou use o modo avançado no viewer."
                    .into(),
            );
        }
        screen
    }

    fn audit(&self, peer_addr: &str, accepted: bool, detail: &str) {
        if let Some(store) = &self.store {
            let _ = store.record_connection(&ConnectionLog {
                peer_addr: peer_addr.to_string(),
                peer_fingerprint: None,
                accepted,
                detail: detail.to_string(),
            });
        }
    }

    fn drain_events(&mut self) {
        while let Some(event) = self.controller.try_next_event() {
            match event {
                HostEvent::CodeReady(code) => {
                    self.code = Some(code);
                    self.session_active = false;
                    self.peer = None;
                    self.pending = None;
                }
                HostEvent::ApprovalRequested(request) => self.pending = Some(request),
                HostEvent::SessionStarted { peer_addr } => {
                    self.session_active = true;
                    self.pending = None;
                    self.audit(&peer_addr, true, "session started");
                    self.peer = Some(peer_addr);
                }
                HostEvent::SessionEnded { reason } => {
                    self.session_active = false;
                    let peer = self.peer.take().unwrap_or_else(|| "unknown".into());
                    self.audit(&peer, false, &format!("session ended: {reason}"));
                    self.push_log(format!("Sessão encerrada: {reason}"));
                }
                HostEvent::Log(line) => {
                    if line.starts_with("rejected")
                        || line.starts_with("blocked")
                        || line.starts_with("declined")
                    {
                        self.audit("unknown", false, &line);
                    }
                    self.push_log(line);
                }
                HostEvent::Error(line) => self.push_log(format!("Erro: {line}")),
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_front(line);
        self.log.truncate(MAX_LOG_LINES);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.drain_events();
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));

        if self.session_active {
            let peer = self.peer.clone().unwrap_or_default();
            egui::Frame::none()
                .fill(Color32::from_rgb(140, 20, 20))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("● SESSÃO REMOTA ATIVA — {peer}"))
                                .color(Color32::WHITE)
                                .strong(),
                        );
                        if ui.button("Encerrar sessão").clicked() {
                            self.controller.command(HostCommand::Stop);
                        }
                    });
                });
            ui.add_space(8.0);
        }

        ui.heading("Modo Host");
        ui.label("Dite o código abaixo para quem vai controlar este computador.");
        ui.add_space(8.0);

        ui.group(|ui| {
            ui.label("Código de acesso (inclui o endereço desta máquina):");
            match &self.code {
                Some(code) => ui.label(RichText::new(code).heading().monospace().strong()),
                None => ui.label(RichText::new("gerando…").italics()),
            };
            ui.add_space(4.0);
            ui.label(format!("Porta: {}", self.controller.local_addr().port()));
            ui.label(format!("Backend: {}", self.backend_label));
            ui.horizontal(|ui| {
                ui.label("Impressão digital (confira por voz):");
            });
            ui.label(RichText::new(self.controller.fingerprint()).monospace().small());
        });

        if let Some(request) = self.pending.clone() {
            ui.add_space(8.0);
            egui::Frame::none()
                .fill(Color32::from_rgb(40, 60, 90))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("Solicitação de conexão de {}", request.peer_addr))
                            .color(Color32::WHITE),
                    );
                    if let Some(fp) = &request.fingerprint {
                        ui.label(RichText::new(fp).monospace().small().color(Color32::WHITE));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Aceitar").clicked() {
                            self.controller.command(HostCommand::Approve);
                        }
                        if ui.button("Recusar").clicked() {
                            self.controller.command(HostCommand::Reject);
                        }
                    });
                });
        }

        ui.add_space(12.0);
        ui.separator();
        ui.label("Registro:");
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            for line in &self.log {
                ui.small(line);
            }
        });
    }
}

/// Viewer mode state machine.
enum ViewerState {
    Idle {
        code_text: String,
        /// Optional `IP:porta` overriding the address embedded in the code
        /// (kept under an "advanced" disclosure for multi-interface hosts).
        addr_override: String,
        error: Option<String>,
    },
    Connecting { rx: mpsc::Receiver<Result<ViewerHandle, ViewerError>> },
    Active(Box<ActiveViewer>),
    Failed(String),
}

impl ViewerState {
    fn idle() -> Self {
        ViewerState::Idle {
            code_text: String::new(),
            addr_override: String::new(),
            error: None,
        }
    }
}

struct ActiveViewer {
    handle: ViewerHandle,
    texture: Option<egui::TextureHandle>,
    tex_size: (usize, usize),
    input: InputState,
    status: String,
    selected_monitor: u32,
}

/// Viewer mode: this machine controls a remote host.
pub struct ViewerScreen {
    runtime: Handle,
    state: ViewerState,
}

impl ViewerScreen {
    pub fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            state: ViewerState::idle(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match &mut self.state {
            ViewerState::Idle { .. } => self.idle_ui(ui),
            ViewerState::Connecting { .. } => self.connecting_ui(ui),
            ViewerState::Active(_) => self.active_ui(ui, ctx),
            ViewerState::Failed(_) => self.failed_ui(ui),
        }
    }

    fn idle_ui(&mut self, ui: &mut egui::Ui) {
        let ViewerState::Idle { code_text, addr_override, error } = &mut self.state else {
            return;
        };
        ui.heading("Modo Viewer");
        ui.label("Digite o código de acesso exibido no computador host.");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Código de acesso:");
            ui.add(
                egui::TextEdit::singleline(code_text)
                    .hint_text("XXXX-XXXX-XXXX-XXXX")
                    .desired_width(220.0)
                    .font(egui::TextStyle::Monospace),
            );
        });

        egui::CollapsingHeader::new("Avançado").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Endereço manual (IP:porta):");
                ui.add(
                    egui::TextEdit::singleline(addr_override)
                        .hint_text("vazio = usar o do código")
                        .desired_width(180.0),
                );
            });
            ui.small("Use apenas se o host tiver várias interfaces e o código apontar para a errada.");
        });

        if let Some(err) = error {
            ui.colored_label(Color32::LIGHT_RED, err.as_str());
        }

        ui.add_space(8.0);
        if ui.button("Conectar").clicked() {
            match Self::resolve_target(code_text, addr_override) {
                Ok((addr, code)) => {
                    let (tx, rx) = mpsc::channel();
                    self.runtime.spawn(async move {
                        let _ = tx.send(connect(addr, code, None).await);
                    });
                    self.state = ViewerState::Connecting { rx };
                }
                Err(message) => *error = Some(message),
            }
        }
    }

    /// Turns the typed access code (plus optional manual address) into the
    /// connection target and the canonical code to authenticate with.
    fn resolve_target(code_text: &str, addr_override: &str) -> Result<(SocketAddr, String), String> {
        let code = security::ConnectCode::parse(code_text.trim()).map_err(|e| {
            format!("Código inválido ({e}). Confira com quem está no computador host.")
        })?;
        let addr = if addr_override.trim().is_empty() {
            code.addr().into()
        } else {
            addr_override
                .trim()
                .parse::<SocketAddr>()
                .map_err(|_| "Endereço manual inválido. Use IP:porta, ex. 192.168.0.10:21118".to_string())?
        };
        Ok((addr, code.as_str().to_string()))
    }

    fn connecting_ui(&mut self, ui: &mut egui::Ui) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        ui.heading("Conectando…");
        ui.spinner();

        let ViewerState::Connecting { rx } = &self.state else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(handle)) => {
                let status = format!(
                    "Conectado. Monitor {}x{}",
                    handle.monitors().first().map(|m| m.width_px).unwrap_or(0),
                    handle.monitors().first().map(|m| m.height_px).unwrap_or(0),
                );
                let selected_monitor = handle
                    .monitors()
                    .iter()
                    .find(|m| m.is_primary)
                    .or_else(|| handle.monitors().first())
                    .map(|m| m.id)
                    .unwrap_or(0);
                self.state = ViewerState::Active(Box::new(ActiveViewer {
                    handle,
                    texture: None,
                    tex_size: (0, 0),
                    input: InputState::default(),
                    status,
                    selected_monitor,
                }));
            }
            Ok(Err(err)) => self.state = ViewerState::Failed(err.to_string()),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.state = ViewerState::Failed("conexão cancelada".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn active_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let ViewerState::Active(active) = &mut self.state else {
            return;
        };
        ctx.request_repaint();

        let mut disconnect = false;
        while let Some(event) = active.handle.try_next_event() {
            match event {
                ViewerEvent::FrameReady => {}
                ViewerEvent::Resized { width_px, height_px } => {
                    active.status = format!("Monitor {width_px}x{height_px}");
                }
                ViewerEvent::MonitorsUpdated(_) => {}
                ViewerEvent::Disconnected(reason) => {
                    self.state = ViewerState::Failed(format!("Desconectado: {reason}"));
                    return;
                }
                ViewerEvent::Error(err) => active.status = format!("Erro: {err}"),
            }
        }

        ui.horizontal(|ui| {
            if ui.button("Desconectar").clicked() {
                active.handle.disconnect();
                disconnect = true;
            }
            ui.label(&active.status);
        });

        if disconnect {
            self.state = ViewerState::Failed("Você encerrou a sessão".into());
            return;
        }

        let monitors = active.handle.monitors().to_vec();
        if monitors.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Monitor:");
                for monitor in &monitors {
                    let label = format!("{} ({}x{})", monitor.name, monitor.width_px, monitor.height_px);
                    let selected = active.selected_monitor == monitor.id;
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        active.selected_monitor = monitor.id;
                        active.handle.select_monitor(monitor.id);
                    }
                }
            });
        }

        active.update_texture(ctx);
        if let Some(texture) = &active.texture {
            let available = ui.available_size();
            let tex_size = egui::vec2(active.tex_size.0 as f32, active.tex_size.1 as f32);
            let display = fit(tex_size, available);
            let (rect, _response) =
                ui.allocate_exact_size(display, egui::Sense::click_and_drag());
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            input_map::forward(ui, rect, &active.handle, &mut active.input);
        } else {
            ui.centered_and_justified(|ui| ui.label("Aguardando primeiro quadro…"));
        }
    }

    fn failed_ui(&mut self, ui: &mut egui::Ui) {
        let ViewerState::Failed(reason) = &self.state else {
            return;
        };
        ui.heading("Sessão encerrada");
        ui.colored_label(Color32::LIGHT_RED, reason.as_str());
        ui.add_space(8.0);
        if ui.button("Voltar").clicked() {
            self.state = ViewerState::idle();
        }
    }
}

impl ActiveViewer {
    fn update_texture(&mut self, ctx: &egui::Context) {
        let frame = match self.handle.frame_slot().lock().ok().and_then(|mut f| f.take()) {
            Some(frame) => frame,
            None => return,
        };
        let size = [frame.width as usize, frame.height as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.data);
        match &mut self.texture {
            Some(texture) if self.tex_size == (size[0], size[1]) => {
                texture.set(image, egui::TextureOptions::LINEAR);
            }
            _ => {
                self.texture = Some(ctx.load_texture("remote_screen", image, egui::TextureOptions::LINEAR));
                self.tex_size = (size[0], size[1]);
            }
        }
    }
}

/// Scales `content` to fit within `bounds` while preserving aspect ratio.
fn fit(content: egui::Vec2, bounds: egui::Vec2) -> egui::Vec2 {
    if content.x <= 0.0 || content.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / content.x).min(bounds.y / content.y).max(0.0);
    content * scale
}
