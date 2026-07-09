use std::env;
use std::net::SocketAddr;

use controlis_server::{app, AppState, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bind_addr: SocketAddr = env::var("CONTROLIS_SERVER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!("controlis rendezvous server listening on {local_addr}");

    axum::serve(
        listener,
        app(AppState::new(ServerConfig::default()))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
