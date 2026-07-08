use std::path::Path;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use crate::TransportError;

/// A SHA-256 fingerprint of a certificate, formatted as colon-separated hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Computes the fingerprint of a DER-encoded certificate.
    pub fn of_der(der: &[u8]) -> Self {
        let digest = Sha256::digest(der);
        let hex: Vec<String> = digest.iter().map(|b| format!("{b:02X}")).collect();
        Self(hex.join(":"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The host's persistent TLS identity: a self-signed certificate, its private key,
/// and the fingerprint the viewer pins.
#[derive(Debug)]
pub struct HostIdentity {
    pub(crate) cert: CertificateDer<'static>,
    pub(crate) key: PrivateKeyDer<'static>,
    fingerprint: Fingerprint,
}

impl Clone for HostIdentity {
    fn clone(&self) -> Self {
        // PrivateKeyDer is not `Clone`; `clone_key` performs the deep copy.
        Self {
            cert: self.cert.clone(),
            key: self.key.clone_key(),
            fingerprint: self.fingerprint.clone(),
        }
    }
}

impl HostIdentity {
    /// Loads the identity from disk, generating and persisting a new one on first run.
    pub fn load_or_generate(cert_path: &Path, key_path: &Path) -> Result<Self, TransportError> {
        match (std::fs::read(cert_path), std::fs::read(key_path)) {
            (Ok(cert_bytes), Ok(key_bytes)) => {
                let cert = CertificateDer::from(cert_bytes);
                let key = PrivateKeyDer::try_from(key_bytes)
                    .map_err(|e| TransportError::Certificate(e.to_string()))?;
                let fingerprint = Fingerprint::of_der(&cert);
                Ok(Self { cert, key, fingerprint })
            }
            _ => {
                let identity = Self::generate()?;
                std::fs::write(cert_path, &identity.cert)?;
                std::fs::write(key_path, identity.key.secret_der())?;
                Ok(identity)
            }
        }
    }

    /// Generates a fresh in-memory identity (used by tests and first run).
    pub fn generate() -> Result<Self, TransportError> {
        let certified = rcgen::generate_simple_self_signed(vec!["controlis-host".to_string()])
            .map_err(|e| TransportError::Certificate(e.to_string()))?;
        let cert = CertificateDer::from(certified.cert);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let fingerprint = Fingerprint::of_der(&cert);
        Ok(Self { cert, key, fingerprint })
    }

    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_formatted() {
        let fp = Fingerprint::of_der(b"hello");
        assert_eq!(fp.as_str().len(), 32 * 2 + 31); // 32 hex pairs + 31 colons
        assert_eq!(fp, Fingerprint::of_der(b"hello"));
        assert_ne!(fp, Fingerprint::of_der(b"world"));
    }

    #[test]
    fn generated_identities_differ() {
        let a = HostIdentity::generate().unwrap();
        let b = HostIdentity::generate().unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
