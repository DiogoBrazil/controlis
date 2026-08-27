//! Controlis desktop app: Tauri backend wiring the session crates to the
//! Leptos webview UI. A single window hosts the host and viewer modes.

mod backend;
mod commands;
mod input_map;

use std::path::PathBuf;

use storage::{AppPaths, Config};
use tauri::Manager;
use transport::{HostIdentity, IrohSecretKey};

use commands::host::{host_command, start_host, stop_host, HostSlot};
use commands::viewer::{
    connect_viewer, viewer_disconnect, viewer_input, viewer_select_monitor, ViewerSlot,
};

/// Immutable app-wide facts loaded at startup.
pub struct AppEnv {
    pub config: Config,
    pub identity: HostIdentity,
    pub iroh_secret_key: IrohSecretKey,
    pub db_path: PathBuf,
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "controlis=info,controlis_lib=info,session_viewer=info,session_host=info,\
                     transport=info,rendezvous=info,wayland_portal=info,warn"
                        .into()
                }),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let paths = AppPaths::discover()?;
            let config = Config::load(&paths.config_file())?;
            let identity = HostIdentity::load_or_generate(
                &paths.certificate_file(),
                &paths.private_key_file(),
            )?;
            let iroh_secret_key =
                transport::load_or_generate_iroh_secret_key(&paths.iroh_secret_key_file())?;
            app.manage(AppEnv {
                config,
                identity,
                iroh_secret_key,
                db_path: paths.database_file(),
            });
            app.manage(HostSlot::default());
            app.manage(ViewerSlot::default());

            // Ctrl+C in a terminal must run the same exit path as closing the
            // window, so sessions and the Wayland portal tear down cleanly.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    handle.exit(0);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_host,
            host_command,
            stop_host,
            connect_viewer,
            viewer_input,
            viewer_select_monitor,
            viewer_disconnect,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Drop sessions before the process ends: the host stops
                // accepting, the viewer disconnects, and PortalHandle::drop
                // closes the Wayland portal session (waiting for the ack).
                if let Some(slot) = app.try_state::<HostSlot>() {
                    drop(slot.0.lock().unwrap().take());
                }
                if let Some(slot) = app.try_state::<ViewerSlot>() {
                    drop(slot.0.lock().unwrap().take());
                }
            }
        });
}
