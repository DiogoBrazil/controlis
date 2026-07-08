use protocol::{ControlMessage, FrameDecoder, MediaMessage};
use quinn::{RecvStream, SendStream};

use crate::{Fingerprint, TransportError};

const CONTROL_READ_CHUNK: usize = 16 * 1024;
/// Upper bound on a single media message read from a unidirectional stream.
const MAX_MEDIA_BYTES: usize = 64 * 1024 * 1024;

/// A live QUIC connection with helpers for the control and media streams.
///
/// Holds an optional owning handle to the endpoint that produced it: on the viewer
/// side the client endpoint must outlive the connection, since quinn drives all
/// connection I/O through the endpoint. Host connections leave this `None` because
/// the [`crate::HostListener`] owns their endpoint.
#[derive(Debug, Clone)]
pub struct Connection {
    inner: quinn::Connection,
    _endpoint: Option<quinn::Endpoint>,
}

impl Connection {
    pub(crate) fn new(inner: quinn::Connection) -> Self {
        Self { inner, _endpoint: None }
    }

    pub(crate) fn with_endpoint(inner: quinn::Connection, endpoint: quinn::Endpoint) -> Self {
        Self { inner, _endpoint: Some(endpoint) }
    }

    /// The peer's certificate fingerprint, derived from the negotiated TLS identity.
    pub fn peer_fingerprint(&self) -> Option<Fingerprint> {
        let identity = self.inner.peer_identity()?;
        let certs = identity.downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>().ok()?;
        certs.first().map(|c| Fingerprint::of_der(c))
    }

    pub fn remote_address(&self) -> std::net::SocketAddr {
        self.inner.remote_address()
    }

    /// Opens the bidirectional control stream (viewer side).
    pub async fn open_control(&self) -> Result<ControlChannel, TransportError> {
        let (send, recv) = self
            .inner
            .open_bi()
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        Ok(ControlChannel::new(send, recv))
    }

    /// Accepts the bidirectional control stream (host side).
    pub async fn accept_control(&self) -> Result<ControlChannel, TransportError> {
        let (send, recv) = self
            .inner
            .accept_bi()
            .await
            .map_err(|_| TransportError::Closed)?;
        Ok(ControlChannel::new(send, recv))
    }

    /// Sends one media message on a fresh unidirectional stream (host -> viewer).
    pub async fn send_media(&self, message: &MediaMessage) -> Result<(), TransportError> {
        let body = protocol::encode(message)?;
        let mut stream = self
            .inner
            .open_uni()
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        stream.write_all(&body).await.map_err(|_| TransportError::Closed)?;
        stream.finish().map_err(|_| TransportError::Closed)?;
        Ok(())
    }

    /// Receives one media message from the next unidirectional stream (viewer side).
    pub async fn recv_media(&self) -> Result<MediaMessage, TransportError> {
        let mut stream = self.inner.accept_uni().await.map_err(|_| TransportError::Closed)?;
        let framed = stream
            .read_to_end(MAX_MEDIA_BYTES)
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        // `encode` prefixes a 4-byte length; on a length-delimited uni stream we
        // already know the boundary, so skip it and decode the body.
        let body = framed.get(4..).ok_or(TransportError::Closed)?;
        Ok(protocol::decode(body)?)
    }

    /// Closes the connection with an application-level reason.
    pub fn close(&self, reason: &str) {
        self.inner.close(0u32.into(), reason.as_bytes());
    }
}

/// The reliable, ordered control stream, carrying length-delimited [`ControlMessage`]s.
///
/// Use directly when send and receive happen in the same task (the host control
/// loop), or [`ControlChannel::split`] it into independent halves when they must
/// run concurrently (the viewer).
#[derive(Debug)]
pub struct ControlChannel {
    sender: ControlSender,
    receiver: ControlReceiver,
}

impl ControlChannel {
    fn new(send: SendStream, recv: RecvStream) -> Self {
        Self {
            sender: ControlSender { send },
            receiver: ControlReceiver {
                recv,
                decoder: FrameDecoder::new(),
            },
        }
    }

    /// Sends a control message.
    pub async fn send(&mut self, message: &ControlMessage) -> Result<(), TransportError> {
        self.sender.send(message).await
    }

    /// Receives the next control message, reading more bytes as needed.
    pub async fn recv(&mut self) -> Result<ControlMessage, TransportError> {
        self.receiver.recv().await
    }

    /// Splits into independent send and receive halves for concurrent use.
    pub fn split(self) -> (ControlSender, ControlReceiver) {
        (self.sender, self.receiver)
    }
}

/// The send half of the control stream.
#[derive(Debug)]
pub struct ControlSender {
    send: SendStream,
}

impl ControlSender {
    pub async fn send(&mut self, message: &ControlMessage) -> Result<(), TransportError> {
        let framed = protocol::encode(message)?;
        self.send
            .write_all(&framed)
            .await
            .map_err(|_| TransportError::Closed)?;
        Ok(())
    }
}

/// The receive half of the control stream.
#[derive(Debug)]
pub struct ControlReceiver {
    recv: RecvStream,
    decoder: FrameDecoder,
}

impl ControlReceiver {
    pub async fn recv(&mut self) -> Result<ControlMessage, TransportError> {
        loop {
            if let Some(body) = self.decoder.next_frame()? {
                return Ok(protocol::decode(&body)?);
            }
            let mut buf = vec![0u8; CONTROL_READ_CHUNK];
            match self
                .recv
                .read(&mut buf)
                .await
                .map_err(|e| TransportError::Connect(e.to_string()))?
            {
                Some(n) => self.decoder.extend(&buf[..n]),
                None => return Err(TransportError::Closed),
            }
        }
    }
}
