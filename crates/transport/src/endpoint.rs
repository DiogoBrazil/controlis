use std::net::SocketAddr;

use quinn::Endpoint;

use crate::{client_config, server_config, Connection, Fingerprint, HostIdentity, SeenFingerprint, TransportError};

/// The DNS name in the host certificate. TOFU ignores it, but QUIC requires a name.
const SERVER_NAME: &str = "controlis-host";

/// A bound host endpoint that accepts incoming viewer connections.
#[derive(Debug)]
pub struct HostListener {
    endpoint: Endpoint,
}

impl HostListener {
    /// Binds a UDP socket and starts listening with the host's TLS identity.
    pub fn bind(addr: SocketAddr, identity: &HostIdentity) -> Result<Self, TransportError> {
        let config = server_config(identity)?;
        let endpoint = Endpoint::server(config, addr)?;
        Ok(Self { endpoint })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.endpoint.local_addr().map_err(TransportError::Io)
    }

    /// Waits for the next incoming connection to complete its handshake.
    ///
    /// Returns `None` when the endpoint is closed.
    pub async fn accept(&self) -> Option<Result<Connection, TransportError>> {
        let incoming = self.endpoint.accept().await?;
        let result = incoming
            .await
            .map(Connection::new)
            .map_err(|e| TransportError::Connect(e.to_string()));
        Some(result)
    }

    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"host shutting down");
    }
}

/// Connects to a host from the viewer side, pinning `expected` if present and
/// recording the presented fingerprint in `seen` for trust-on-first-use.
pub async fn connect_viewer(
    addr: SocketAddr,
    expected: Option<Fingerprint>,
    seen: SeenFingerprint,
) -> Result<Connection, TransportError> {
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse().expect("valid bind addr"))?;
    endpoint.set_default_client_config(client_config(expected, seen)?);
    let connection = endpoint
        .connect(addr, SERVER_NAME)
        .map_err(|e| TransportError::Connect(e.to_string()))?
        .await
        .map_err(|e| TransportError::Connect(e.to_string()))?;
    Ok(Connection::with_endpoint(connection, endpoint))
}
