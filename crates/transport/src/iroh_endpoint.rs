use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;

use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr};

use crate::{Connection, TransportError};

const CONTROLIS_ALPN: &[u8] = b"dev.controlis/session/1";
const ONLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrohEndpointDescriptor {
    pub endpoint_id: String,
    pub relay_url: Option<String>,
    pub direct_addrs: Vec<String>,
}

#[derive(Debug)]
pub struct IrohListener {
    endpoint: Endpoint,
    relay_url: String,
}

impl IrohListener {
    pub async fn bind(secret_key: SecretKey, relay_url: &str) -> Result<Self, TransportError> {
        let relay_map = relay_map(relay_url)?;
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(secret_key)
            .alpns(vec![CONTROLIS_ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(relay_map))
            .bind()
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online())
            .await
            .map_err(|_| TransportError::Connect("Iroh endpoint did not become online".into()))?;
        Ok(Self {
            endpoint,
            relay_url: relay_url.into(),
        })
    }

    pub fn descriptor(&self) -> IrohEndpointDescriptor {
        let addr = self.endpoint.addr();
        let relay_url = addr
            .relay_urls()
            .next()
            .map(|url| url.to_string())
            .or_else(|| Some(self.relay_url.clone()));
        let direct_addrs = addr.ip_addrs().map(|addr| addr.to_string()).collect();
        IrohEndpointDescriptor {
            endpoint_id: self.endpoint.id().to_string(),
            relay_url,
            direct_addrs,
        }
    }

    pub async fn accept(&self) -> Option<Result<Connection, TransportError>> {
        loop {
            let incoming = self.endpoint.accept().await?;
            let mut accepting = match incoming.accept() {
                Ok(accepting) => accepting,
                Err(e) => return Some(Err(TransportError::Connect(e.to_string()))),
            };
            let alpn = match accepting.alpn().await {
                Ok(alpn) => alpn,
                Err(e) => return Some(Err(TransportError::Connect(e.to_string()))),
            };
            if alpn != CONTROLIS_ALPN {
                continue;
            }
            return Some(
                accepting
                    .await
                    .map(Connection::from_iroh)
                    .map_err(|e| TransportError::Connect(e.to_string())),
            );
        }
    }
}

pub async fn connect_iroh(
    secret_key: SecretKey,
    relay_url: &str,
    descriptor: &IrohEndpointDescriptor,
) -> Result<Connection, TransportError> {
    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![CONTROLIS_ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relay_map(relay_url)?))
        .bind()
        .await
        .map_err(|e| TransportError::Connect(e.to_string()))?;
    tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online())
        .await
        .map_err(|_| TransportError::Connect("Iroh endpoint did not become online".into()))?;

    let endpoint_id = descriptor
        .endpoint_id
        .parse()
        .map_err(|e| TransportError::Connect(format!("invalid Iroh endpoint id: {e}")))?;
    let relay_url = descriptor
        .relay_url
        .as_deref()
        .unwrap_or(relay_url)
        .parse::<RelayUrl>()
        .map_err(|e| TransportError::Connect(format!("invalid relay URL: {e}")))?;
    let addrs = descriptor
        .direct_addrs
        .iter()
        .filter_map(|addr| addr.parse::<SocketAddr>().ok())
        .map(TransportAddr::Ip)
        .chain(std::iter::once(TransportAddr::Relay(relay_url)));
    let addr = EndpointAddr::from_parts(endpoint_id, addrs);

    let connection = endpoint
        .connect(addr, CONTROLIS_ALPN)
        .await
        .map_err(|e| TransportError::Connect(e.to_string()))?;
    Ok(Connection::from_iroh_with_endpoint(connection, endpoint))
}

pub fn load_or_generate_secret_key(path: &Path) -> Result<SecretKey, TransportError> {
    match std::fs::read_to_string(path) {
        Ok(text) => SecretKey::from_str(text.trim())
            .map_err(|e| TransportError::Certificate(format!("invalid Iroh secret key: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let secret_key = SecretKey::generate();
            std::fs::write(path, hex::encode(secret_key.to_bytes()))?;
            Ok(secret_key)
        }
        Err(e) => Err(e.into()),
    }
}

fn relay_map(relay_url: &str) -> Result<RelayMap, TransportError> {
    RelayMap::try_from_iter([relay_url])
        .map_err(|e| TransportError::Connect(format!("invalid relay URL: {e}")))
}
