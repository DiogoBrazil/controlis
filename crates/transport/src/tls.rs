use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::{Fingerprint, HostIdentity, TransportError};

/// ALPN token; the version suffix lets future protocol revisions coexist.
const ALPN: &[u8] = b"controlis/1";

/// Shared slot the viewer reads after a handshake to learn the host's fingerprint
/// (for trust-on-first-use persistence).
pub type SeenFingerprint = Arc<Mutex<Option<Fingerprint>>>;

fn install_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = ring::default_provider().install_default();
    });
}

/// Builds the host's QUIC server configuration from its persistent identity.
pub fn server_config(identity: &HostIdentity) -> Result<quinn::ServerConfig, TransportError> {
    install_provider();
    let mut crypto = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(quic));
    server.transport_config(transport_config());
    Ok(server)
}

/// Shared QUIC transport tuning: keep-alive to survive NAT/idle, and a bounded
/// idle timeout so a dead peer is detected instead of lingering.
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(Duration::from_secs(5)));
    config.max_idle_timeout(Some(
        Duration::from_secs(30).try_into().expect("valid idle timeout"),
    ));
    Arc::new(config)
}

/// Builds the viewer's QUIC client configuration.
///
/// `expected` is the pinned fingerprint from a previous session, if any. On first
/// contact it is `None` and the presented certificate is accepted but recorded in
/// `seen` so the app layer can persist it; on later sessions a mismatch aborts the
/// handshake.
pub fn client_config(
    expected: Option<Fingerprint>,
    seen: SeenFingerprint,
) -> Result<quinn::ClientConfig, TransportError> {
    install_provider();
    let provider = Arc::new(ring::default_provider());
    let verifier = Arc::new(TofuVerifier {
        provider: provider.clone(),
        expected,
        seen,
    });
    let mut crypto = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let quic = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    let mut client = quinn::ClientConfig::new(Arc::new(quic));
    client.transport_config(transport_config());
    Ok(client)
}

/// Trust-on-first-use certificate verifier.
///
/// Self-signed peer certs never chain to a public CA, so ordinary WebPKI
/// validation is intentionally bypassed. Trust instead rests on pinning the
/// fingerprint across sessions.
#[derive(Debug)]
struct TofuVerifier {
    provider: Arc<CryptoProvider>,
    expected: Option<Fingerprint>,
    seen: SeenFingerprint,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = Fingerprint::of_der(end_entity);
        if let Some(expected) = &self.expected {
            if presented != *expected {
                return Err(rustls::Error::General(
                    "host certificate fingerprint does not match the pinned value".into(),
                ));
            }
        }
        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some(presented);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
