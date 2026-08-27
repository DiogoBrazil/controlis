//! Host-side session orchestration: accept a viewer, authenticate it, then run the
//! capture -> encode -> send loop and apply incoming input.
//!
//! The host runs one session at a time (the MVP scope). Backends are injected as
//! factories so the pipeline can be driven with the synthetic capturer and a mock
//! injector in tests, with no display or OS input access.

mod event;
mod session;

pub use event::{ConnectionRequest, HostCommand, HostEvent};

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use capture::{CaptureError, ScreenCapturer};
use input::{InputError, InputInjector};
use protocol::VideoCodec;
use rendezvous::{
    Client as RendezvousClient, EndpointDescriptor, RefreshRequest, RegisterRequest, SessionId,
    UnregisterRequest, DEFAULT_TTL_SECS,
};
use security::{BruteForceGuard, ConnectCode};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use transport::{HostIdentity, HostListener, IrohListener, IrohSecretKey};

/// Creates a fresh screen capturer for a session (called once per session).
pub type CapturerFactory =
    Arc<dyn Fn() -> Result<Box<dyn ScreenCapturer>, CaptureError> + Send + Sync>;

/// Creates a fresh input injector on the input thread (injectors are not `Send`).
pub type InjectorFactory =
    Arc<dyn Fn() -> Result<Box<dyn InputInjector>, InputError> + Send + Sync>;

/// Configuration for the host service.
#[derive(Debug, Clone)]
pub struct HostConfig {
    pub bind_addr: SocketAddr,
    pub require_manual_approval: bool,
    /// Preferred codec. Each session negotiates the actual codec against the
    /// viewer's advertised support, falling back to JPEG tiles.
    pub codec: VideoCodec,
    pub target_fps: u32,
    /// LAN IPv4 embedded in the access code shown to the user (the bind address
    /// is usually `0.0.0.0` and says nothing about how peers reach this host).
    /// `None` falls back to loopback, which only works for same-machine tests.
    pub advertised_ip: Option<Ipv4Addr>,
    pub internet: Option<InternetHostConfig>,
}

#[derive(Debug, Clone)]
pub struct InternetHostConfig {
    pub rendezvous_url: String,
    pub relay_url: String,
    pub secret_key: IrohSecretKey,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:21118".parse().expect("valid addr"),
            require_manual_approval: true,
            codec: VideoCodec::JpegTiles,
            target_fps: 20,
            advertised_ip: None,
            internet: None,
        }
    }
}

/// Errors that prevent the host service from starting.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Transport(#[from] transport::TransportError),
}

/// Handle the UI holds to observe and control the running host.
#[derive(Debug)]
pub struct HostController {
    events: mpsc::UnboundedReceiver<HostEvent>,
    commands: mpsc::UnboundedSender<HostCommand>,
    fingerprint: String,
    local_addr: SocketAddr,
}

impl HostController {
    /// Receives the next host event, or `None` once the host has stopped.
    pub async fn next_event(&mut self) -> Option<HostEvent> {
        self.events.recv().await
    }

    /// Non-blocking event poll for synchronous UI loops.
    pub fn try_next_event(&mut self) -> Option<HostEvent> {
        self.events.try_recv().ok()
    }

    /// Sends a command to the host (approve/reject/stop).
    pub fn command(&self, command: HostCommand) {
        let _ = self.commands.send(command);
    }

    /// A clonable command sender, for UIs where the controller itself is owned
    /// by an event-pump task.
    pub fn command_sender(&self) -> mpsc::UnboundedSender<HostCommand> {
        self.commands.clone()
    }

    /// The host's certificate fingerprint, for the user to read aloud.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The actual bound address (useful when the config used port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// Starts the host: binds the endpoint and spawns the accept/session loop.
pub fn start(
    config: HostConfig,
    identity: HostIdentity,
    capturer_factory: CapturerFactory,
    injector_factory: InjectorFactory,
) -> Result<HostController, HostError> {
    let listener = HostListener::bind(config.bind_addr, &identity)?;
    let local_addr = listener.local_addr()?;
    let fingerprint = identity.fingerprint().to_string();

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    let advertised_addr = SocketAddrV4::new(
        config.advertised_ip.unwrap_or(Ipv4Addr::LOCALHOST),
        local_addr.port(),
    );

    tokio::spawn(run(
        config,
        listener,
        advertised_addr,
        capturer_factory,
        injector_factory,
        event_tx,
        command_rx,
    ));

    Ok(HostController {
        events: event_rx,
        commands: command_tx,
        fingerprint,
        local_addr,
    })
}

async fn run(
    config: HostConfig,
    listener: HostListener,
    advertised_addr: SocketAddrV4,
    capturer_factory: CapturerFactory,
    injector_factory: InjectorFactory,
    event_tx: mpsc::UnboundedSender<HostEvent>,
    mut command_rx: mpsc::UnboundedReceiver<HostCommand>,
) {
    let mut guard = BruteForceGuard::new();

    loop {
        let internet_registration = register_internet(&config, &event_tx).await;
        let code = match &internet_registration {
            Some(registration) => registration.code.clone(),
            None => ConnectCode::generate(advertised_addr),
        };
        if event_tx
            .send(HostEvent::CodeReady(code.as_str().to_string()))
            .is_err()
        {
            return;
        }

        let connection = match accept_any(&listener, internet_registration.as_ref()).await {
            Some(Ok(conn)) => conn,
            Some(Err(e)) => {
                let _ = event_tx.send(HostEvent::Error(format!("accept failed: {e}")));
                unregister_internet(internet_registration).await;
                continue;
            }
            None => {
                unregister_internet(internet_registration).await;
                return;
            }
        };

        session::run_session(
            &config,
            connection,
            &code,
            &mut guard,
            &capturer_factory,
            &injector_factory,
            &event_tx,
            &mut command_rx,
        )
        .await;
        unregister_internet(internet_registration).await;
    }
}

#[derive(Debug)]
struct InternetRegistration {
    client: RendezvousClient,
    listener: IrohListener,
    session_id: SessionId,
    token: String,
    code: ConnectCode,
    refresh_task: JoinHandle<()>,
}

async fn register_internet(
    config: &HostConfig,
    event_tx: &mpsc::UnboundedSender<HostEvent>,
) -> Option<InternetRegistration> {
    let internet = config.internet.as_ref()?;
    let client = match RendezvousClient::new(&internet.rendezvous_url) {
        Ok(client) => client,
        Err(e) => {
            log_internet_fallback(event_tx, format!("rendezvous inválido: {e}"));
            return None;
        }
    };
    let listener = match IrohListener::bind(internet.secret_key.clone(), &internet.relay_url).await
    {
        Ok(listener) => listener,
        Err(e) => {
            log_internet_fallback(event_tx, format!("transporte Iroh indisponível: {e}"));
            return None;
        }
    };
    let session_id = SessionId::generate();
    let code = match ConnectCode::generate_rendezvous(session_id.get()) {
        Ok(code) => code,
        Err(e) => {
            log_internet_fallback(event_tx, format!("código internet indisponível: {e}"));
            return None;
        }
    };
    let endpoint = endpoint_descriptor(listener.descriptor());
    let response = match client
        .register(&RegisterRequest {
            session_id,
            endpoint,
            ttl_secs: Some(DEFAULT_TTL_SECS),
        })
        .await
    {
        Ok(response) => response,
        Err(e) => {
            log_internet_fallback(event_tx, format!("registro rendezvous falhou: {e}"));
            return None;
        }
    };
    let _ = event_tx.send(HostEvent::Log(format!(
        "internet ativo: rendezvous {}, relay {}",
        internet.rendezvous_url, internet.relay_url
    )));
    let refresh_task = spawn_refresh_task(
        client.clone(),
        session_id,
        response.token.clone(),
        endpoint_descriptor(listener.descriptor()),
    );
    Some(InternetRegistration {
        client,
        listener,
        session_id,
        token: response.token,
        code,
        refresh_task,
    })
}

fn log_internet_fallback(event_tx: &mpsc::UnboundedSender<HostEvent>, reason: String) {
    tracing::warn!("internet disabled: {reason}");
    let _ = event_tx.send(HostEvent::Log(format!(
        "internet indisponível ({reason}); usando código LAN"
    )));
}

async fn accept_any(
    lan: &HostListener,
    internet: Option<&InternetRegistration>,
) -> Option<Result<transport::Connection, transport::TransportError>> {
    match internet {
        Some(internet) => {
            tokio::select! {
                lan = lan.accept() => lan,
                iroh = internet.listener.accept() => iroh,
            }
        }
        None => lan.accept().await,
    }
}

async fn unregister_internet(registration: Option<InternetRegistration>) {
    let Some(registration) = registration else {
        return;
    };
    registration.refresh_task.abort();
    let _ = registration
        .client
        .unregister(&UnregisterRequest {
            session_id: registration.session_id,
            token: registration.token,
        })
        .await;
}

fn spawn_refresh_task(
    client: RendezvousClient,
    session_id: SessionId,
    token: String,
    endpoint: EndpointDescriptor,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(DEFAULT_TTL_SECS / 2));
        loop {
            interval.tick().await;
            if let Err(e) = client
                .refresh(&RefreshRequest {
                    session_id,
                    token: token.clone(),
                    endpoint: endpoint.clone(),
                    ttl_secs: Some(DEFAULT_TTL_SECS),
                })
                .await
            {
                tracing::warn!("internet rendezvous refresh failed: {e}");
            }
        }
    })
}

fn endpoint_descriptor(descriptor: transport::IrohEndpointDescriptor) -> EndpointDescriptor {
    EndpointDescriptor {
        endpoint_id: descriptor.endpoint_id,
        relay_url: descriptor.relay_url,
        direct_addrs: descriptor.direct_addrs,
    }
}
