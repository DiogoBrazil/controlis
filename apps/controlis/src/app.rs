use std::sync::Arc;

use storage::{AppPaths, Config};
use transport::HostIdentity;

use crate::screens::{HostScreen, ViewerScreen};

/// Which screen the app is showing.
enum Screen {
    Home,
    Host(Box<HostScreen>),
    Viewer(Box<ViewerScreen>),
}

/// The top-level eframe application.
pub struct ControlisApp {
    paths: Arc<AppPaths>,
    config: Config,
    identity: HostIdentity,
    runtime: tokio::runtime::Runtime,
    screen: Screen,
}

impl ControlisApp {
    pub fn new(
        paths: Arc<AppPaths>,
        config: Config,
        identity: HostIdentity,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        Self {
            paths,
            config,
            identity,
            runtime,
            screen: Screen::Home,
        }
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.heading("Controlis");
            ui.label("Acesso remoto seguro — escolha um modo:");
            ui.add_space(24.0);

            if ui
                .add_sized([260.0, 48.0], egui::Button::new("🖥  Permitir controle (Host)"))
                .clicked()
            {
                let screen = HostScreen::start(
                    self.runtime.handle().clone(),
                    &self.config,
                    self.identity.clone(),
                    &self.paths.database_file(),
                );
                self.screen = Screen::Host(Box::new(screen));
            }
            ui.add_space(12.0);
            if ui
                .add_sized([260.0, 48.0], egui::Button::new("👁  Controlar outro (Viewer)"))
                .clicked()
            {
                let default_port = self.config.host_port;
                self.screen = Screen::Viewer(Box::new(ViewerScreen::new(
                    self.runtime.handle().clone(),
                    default_port,
                )));
            }
        });

        ui.add_space(24.0);
        ui.separator();
        ui.small(format!("Config: {}", self.paths.config_file().display()));
    }
}

impl eframe::App for ControlisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let at_home = matches!(self.screen, Screen::Home);
                if !at_home && ui.button("← Início").clicked() {
                    self.screen = Screen::Home;
                }
                ui.separator();
                let mode = match &self.screen {
                    Screen::Home => "Início",
                    Screen::Host(_) => "Host",
                    Screen::Viewer(_) => "Viewer",
                };
                ui.label(mode);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut render_home = false;
            match &mut self.screen {
                Screen::Home => render_home = true,
                Screen::Host(screen) => screen.ui(ui),
                Screen::Viewer(screen) => screen.ui(ui, ctx),
            }
            if render_home {
                self.home_ui(ui);
            }
        });
    }
}
