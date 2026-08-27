//! Shared rendezvous protocol for internet sessions.
//!
//! The rendezvous server only stores temporary reachability metadata. The
//! session secret remains in the access code and endpoint traffic stays
//! end-to-end encrypted by the transport layer.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};

const SESSION_ID_BITS: u32 = 48;
const SESSION_ID_MASK: u64 = (1u64 << SESSION_ID_BITS) - 1;
const TOKEN_BYTES: usize = 16;

pub const DEFAULT_TTL_SECS: u64 = 60;
pub const MAX_TTL_SECS: u64 = 120;

/// A short-lived rendezvous registration id embedded in an internet access code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(u64);

impl SessionId {
    pub fn generate() -> Self {
        Self(rand::thread_rng().gen_range(0..=SESSION_ID_MASK))
    }

    pub fn new(id: u64) -> Result<Self, RendezvousError> {
        if id > SESSION_ID_MASK {
            return Err(RendezvousError::SessionIdOutOfRange);
        }
        Ok(Self(id))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Reachability data published by a host.
///
/// These strings intentionally keep Iroh-specific parsing out of the shared
/// protocol crate while the transport layer is still being introduced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointDescriptor {
    pub endpoint_id: String,
    pub relay_url: Option<String>,
    #[serde(default)]
    pub direct_addrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub session_id: SessionId,
    pub endpoint: EndpointDescriptor,
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub token: String,
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub session_id: SessionId,
    pub token: String,
    pub endpoint: EndpointDescriptor,
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResponse {
    pub endpoint: EndpointDescriptor,
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnregisterRequest {
    pub session_id: SessionId,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RendezvousError {
    #[error("session id must fit in {SESSION_ID_BITS} bits")]
    SessionIdOutOfRange,
    #[error("session id is already registered")]
    AlreadyRegistered,
    #[error("session id is not registered")]
    NotFound,
    #[error("registration token does not match")]
    TokenMismatch,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid rendezvous server URL")]
    InvalidBaseUrl,
    #[error("rendezvous request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("rendezvous server returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Result<Self, ClientError> {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        if base_url.is_empty()
            || !(base_url.starts_with("http://") || base_url.starts_with("https://"))
        {
            return Err(ClientError::InvalidBaseUrl);
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
        })
    }

    pub async fn register(
        &self,
        request: &RegisterRequest,
    ) -> Result<RegisterResponse, ClientError> {
        self.post_json("/v1/register", request).await
    }

    pub async fn refresh(&self, request: &RefreshRequest) -> Result<RegisterResponse, ClientError> {
        self.post_json("/v1/refresh", request).await
    }

    pub async fn lookup(&self, session_id: SessionId) -> Result<LookupResponse, ClientError> {
        let response = self
            .http
            .get(format!(
                "{}/v1/sessions/{}",
                self.base_url,
                session_id.get()
            ))
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn unregister(&self, request: &UnregisterRequest) -> Result<(), ClientError> {
        let response = self
            .http
            .post(format!("{}/v1/unregister", self.base_url))
            .json(request)
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(response).await)
        }
    }

    async fn post_json<T, R>(&self, path: &str, request: &T) -> Result<R, ClientError>
    where
        T: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }
}

async fn decode_response<T>(response: reqwest::Response) -> Result<T, ClientError>
where
    T: for<'de> Deserialize<'de>,
{
    if response.status().is_success() {
        Ok(response.json().await?)
    } else {
        Err(status_error(response).await)
    }
}

async fn status_error(response: reqwest::Response) -> ClientError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ClientError::Status { status, body }
}

#[derive(Debug, Clone)]
struct Registration {
    token: String,
    endpoint: EndpointDescriptor,
    expires_at: Instant,
    expires_at_unix: u64,
}

/// In-memory rendezvous store with bounded TTL.
#[derive(Debug, Default)]
pub struct RendezvousStore {
    entries: HashMap<SessionId, Registration>,
}

impl RendezvousStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        request: RegisterRequest,
    ) -> Result<RegisterResponse, RendezvousError> {
        let now = Instant::now();
        self.register_at(request, now, unix_now())
    }

    pub fn refresh(
        &mut self,
        request: RefreshRequest,
    ) -> Result<RegisterResponse, RendezvousError> {
        let now = Instant::now();
        self.refresh_at(request, now, unix_now())
    }

    pub fn lookup(&mut self, session_id: SessionId) -> Result<LookupResponse, RendezvousError> {
        self.lookup_at(session_id, Instant::now())
    }

    pub fn unregister(&mut self, request: UnregisterRequest) -> Result<(), RendezvousError> {
        self.purge_expired_at(Instant::now());
        let Some(existing) = self.entries.get(&request.session_id) else {
            return Err(RendezvousError::NotFound);
        };
        if existing.token != request.token {
            return Err(RendezvousError::TokenMismatch);
        }
        self.entries.remove(&request.session_id);
        Ok(())
    }

    pub fn len(&mut self) -> usize {
        self.purge_expired_at(Instant::now());
        self.entries.len()
    }

    pub fn is_empty(&mut self) -> bool {
        self.len() == 0
    }

    fn register_at(
        &mut self,
        request: RegisterRequest,
        now: Instant,
        now_unix: u64,
    ) -> Result<RegisterResponse, RendezvousError> {
        self.purge_expired_at(now);
        if self.entries.contains_key(&request.session_id) {
            return Err(RendezvousError::AlreadyRegistered);
        }
        let token = generate_token();
        let expires_at = now + ttl(request.ttl_secs);
        let expires_at_unix = now_unix + ttl(request.ttl_secs).as_secs();
        self.entries.insert(
            request.session_id,
            Registration {
                token: token.clone(),
                endpoint: request.endpoint,
                expires_at,
                expires_at_unix,
            },
        );
        Ok(RegisterResponse {
            token,
            expires_at_unix,
        })
    }

    fn refresh_at(
        &mut self,
        request: RefreshRequest,
        now: Instant,
        now_unix: u64,
    ) -> Result<RegisterResponse, RendezvousError> {
        self.purge_expired_at(now);
        let Some(existing) = self.entries.get_mut(&request.session_id) else {
            return Err(RendezvousError::NotFound);
        };
        if existing.token != request.token {
            return Err(RendezvousError::TokenMismatch);
        }
        existing.endpoint = request.endpoint;
        existing.expires_at = now + ttl(request.ttl_secs);
        existing.expires_at_unix = now_unix + ttl(request.ttl_secs).as_secs();
        Ok(RegisterResponse {
            token: existing.token.clone(),
            expires_at_unix: existing.expires_at_unix,
        })
    }

    fn lookup_at(
        &mut self,
        session_id: SessionId,
        now: Instant,
    ) -> Result<LookupResponse, RendezvousError> {
        self.purge_expired_at(now);
        let Some(existing) = self.entries.get(&session_id) else {
            return Err(RendezvousError::NotFound);
        };
        Ok(LookupResponse {
            endpoint: existing.endpoint.clone(),
            expires_at_unix: existing.expires_at_unix,
        })
    }

    fn purge_expired_at(&mut self, now: Instant) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }
}

fn ttl(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(DEFAULT_TTL_SECS).min(MAX_TTL_SECS))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(id: &str) -> EndpointDescriptor {
        EndpointDescriptor {
            endpoint_id: id.into(),
            relay_url: Some("https://relay.example.test".into()),
            direct_addrs: vec!["127.0.0.1:12345".into()],
        }
    }

    fn register_request(session_id: SessionId, endpoint_id: &str) -> RegisterRequest {
        RegisterRequest {
            session_id,
            endpoint: endpoint(endpoint_id),
            ttl_secs: Some(30),
        }
    }

    #[test]
    fn session_id_is_limited_to_48_bits() {
        assert_eq!(
            SessionId::new((1u64 << 48) - 1).unwrap().get(),
            (1u64 << 48) - 1
        );
        assert_eq!(
            SessionId::new(1u64 << 48),
            Err(RendezvousError::SessionIdOutOfRange)
        );
    }

    #[test]
    fn register_and_lookup_roundtrip() {
        let mut store = RendezvousStore::new();
        let session_id = SessionId::new(42).unwrap();
        let response = store
            .register(register_request(session_id, "host-a"))
            .unwrap();
        assert_eq!(response.token.len(), TOKEN_BYTES * 2);

        let lookup = store.lookup(session_id).unwrap();
        assert_eq!(lookup.endpoint.endpoint_id, "host-a");
        assert_eq!(lookup.expires_at_unix, response.expires_at_unix);
    }

    #[test]
    fn registration_rejects_active_collision() {
        let mut store = RendezvousStore::new();
        let session_id = SessionId::new(7).unwrap();
        store
            .register(register_request(session_id, "host-a"))
            .unwrap();

        assert_eq!(
            store.register(register_request(session_id, "host-b")),
            Err(RendezvousError::AlreadyRegistered)
        );
    }

    #[test]
    fn refresh_requires_matching_token_and_updates_endpoint() {
        let mut store = RendezvousStore::new();
        let session_id = SessionId::new(9).unwrap();
        let registered = store
            .register(register_request(session_id, "host-a"))
            .unwrap();

        assert_eq!(
            store.refresh(RefreshRequest {
                session_id,
                token: "wrong".into(),
                endpoint: endpoint("host-b"),
                ttl_secs: Some(30),
            }),
            Err(RendezvousError::TokenMismatch)
        );

        let refreshed = store
            .refresh(RefreshRequest {
                session_id,
                token: registered.token.clone(),
                endpoint: endpoint("host-b"),
                ttl_secs: Some(30),
            })
            .unwrap();
        assert_eq!(refreshed.token, registered.token);
        assert_eq!(
            store.lookup(session_id).unwrap().endpoint.endpoint_id,
            "host-b"
        );
    }

    #[test]
    fn unregister_requires_matching_token() {
        let mut store = RendezvousStore::new();
        let session_id = SessionId::new(11).unwrap();
        let registered = store
            .register(register_request(session_id, "host-a"))
            .unwrap();

        assert_eq!(
            store.unregister(UnregisterRequest {
                session_id,
                token: "wrong".into(),
            }),
            Err(RendezvousError::TokenMismatch)
        );

        store
            .unregister(UnregisterRequest {
                session_id,
                token: registered.token,
            })
            .unwrap();
        assert_eq!(store.lookup(session_id), Err(RendezvousError::NotFound));
    }

    #[test]
    fn expired_registration_can_be_reused() {
        let mut store = RendezvousStore::new();
        let session_id = SessionId::new(13).unwrap();
        let now = Instant::now();
        store
            .register_at(
                RegisterRequest {
                    session_id,
                    endpoint: endpoint("host-a"),
                    ttl_secs: Some(1),
                },
                now,
                1_000,
            )
            .unwrap();

        assert_eq!(
            store.lookup_at(session_id, now + Duration::from_secs(1)),
            Err(RendezvousError::NotFound)
        );
        store
            .register_at(
                register_request(session_id, "host-b"),
                now + Duration::from_secs(2),
                1_002,
            )
            .unwrap();
        assert_eq!(
            store.lookup(session_id).unwrap().endpoint.endpoint_id,
            "host-b"
        );
    }
}
