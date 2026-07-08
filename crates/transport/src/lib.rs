//! QUIC transport (via `quinn` + `rustls`) carrying the [`protocol`] messages.
//!
//! TLS 1.3 is mandatory — there is no cleartext path. Host identity is a
//! persistent self-signed certificate; the viewer pins it trust-on-first-use.
//! A single connection carries a bidirectional control stream and any number of
//! unidirectional media streams (host -> viewer).

mod connection;
mod endpoint;
mod identity;
mod tls;

pub use connection::{Connection, ControlChannel, ControlReceiver, ControlSender};
pub use endpoint::{connect_viewer, HostListener};
pub use identity::{Fingerprint, HostIdentity};
pub use tls::{client_config, server_config, SeenFingerprint};

/// Errors from establishing or using the transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("tls setup failed: {0}")]
    Tls(String),
    #[error("certificate error: {0}")]
    Certificate(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("stream closed by peer")]
    Closed,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] protocol::ProtocolError),
}
