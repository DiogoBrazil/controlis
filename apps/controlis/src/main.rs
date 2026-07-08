//! Controlis desktop app: a single binary with a host mode (this machine is
//! controlled) and a viewer mode (this machine controls another).

mod app;
mod factories;
mod input_map;
mod screens;

use std::sync::Arc;

use anyhow::Context;
use storage::{AppPaths, Config};
use transport::HostIdentity;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "controlis=info,warn".into()),
        )
        .init();

    let paths = AppPaths::discover().context("resolving app directories")?;
    let config = Config::load(&paths.config_file()).context("loading config")?;
    let identity = HostIdentity::load_or_generate(&paths.certificate_file(), &paths.private_key_file())
        .context("loading host identity")?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building async runtime")?;

    let app = app::ControlisApp::new(Arc::new(paths), config, identity, runtime);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Controlis")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Controlis",
        native_options,
        Box::new(|cc| {
            app.install_ctrl_c_handler(cc.egui_ctx.clone());
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}
