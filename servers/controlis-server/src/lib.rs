use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rendezvous::{
    LookupResponse, RefreshRequest, RegisterRequest, RegisterResponse, RendezvousError,
    RendezvousStore, SessionId, UnregisterRequest,
};
use serde::Serialize;

const DEFAULT_MAX_REQUESTS_PER_MINUTE: u32 = 120;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_requests_per_minute: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_requests_per_minute: DEFAULT_MAX_REQUESTS_PER_MINUTE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    store: Arc<Mutex<RendezvousStore>>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    config: ServerConfig,
}

impl AppState {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            store: Arc::new(Mutex::new(RendezvousStore::new())),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            config,
        }
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/register", post(register))
        .route("/v1/refresh", post(refresh))
        .route("/v1/sessions/{session_id}", get(lookup))
        .route("/v1/unregister", post(unregister))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn register(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    check_rate_limit(&state, addr)?;
    let response = state
        .store
        .lock()
        .expect("rendezvous store poisoned")
        .register(request)?;
    Ok(Json(response))
}

async fn refresh(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    check_rate_limit(&state, addr)?;
    let response = state
        .store
        .lock()
        .expect("rendezvous store poisoned")
        .refresh(request)?;
    Ok(Json(response))
}

async fn lookup(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Path(session_id): Path<u64>,
) -> Result<Json<LookupResponse>, ApiError> {
    check_rate_limit(&state, addr)?;
    let session_id = SessionId::new(session_id)?;
    let response = state
        .store
        .lock()
        .expect("rendezvous store poisoned")
        .lookup(session_id)?;
    Ok(Json(response))
}

async fn unregister(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(request): Json<UnregisterRequest>,
) -> Result<StatusCode, ApiError> {
    check_rate_limit(&state, addr)?;
    state
        .store
        .lock()
        .expect("rendezvous store poisoned")
        .unregister(request)?;
    Ok(StatusCode::NO_CONTENT)
}

fn check_rate_limit(state: &AppState, addr: SocketAddr) -> Result<(), ApiError> {
    let mut limiter = state.rate_limiter.lock().expect("rate limiter poisoned");
    if limiter.check(addr.ip(), state.config.max_requests_per_minute) {
        Ok(())
    } else {
        Err(ApiError::RateLimited)
    }
}

#[derive(Debug, Default)]
struct RateLimiter {
    buckets: HashMap<IpAddr, RateBucket>,
}

impl RateLimiter {
    fn check(&mut self, ip: IpAddr, max_requests: u32) -> bool {
        if max_requests == 0 {
            return false;
        }
        let now = Instant::now();
        self.buckets.retain(|_, bucket| {
            now.duration_since(bucket.window_started) < Duration::from_secs(60)
        });
        let bucket = self.buckets.entry(ip).or_insert(RateBucket {
            window_started: now,
            count: 0,
        });
        if now.duration_since(bucket.window_started) >= Duration::from_secs(60) {
            bucket.window_started = now;
            bucket.count = 0;
        }
        if bucket.count >= max_requests {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[derive(Debug)]
struct RateBucket {
    window_started: Instant,
    count: u32,
}

#[derive(Debug)]
enum ApiError {
    Rendezvous(RendezvousError),
    RateLimited,
}

impl From<RendezvousError> for ApiError {
    fn from(value: RendezvousError) -> Self {
        Self::Rendezvous(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Rendezvous(RendezvousError::SessionIdOutOfRange) => {
                (StatusCode::BAD_REQUEST, "session id is out of range")
            }
            Self::Rendezvous(RendezvousError::AlreadyRegistered) => {
                (StatusCode::CONFLICT, "session id is already registered")
            }
            Self::Rendezvous(RendezvousError::NotFound) => {
                (StatusCode::NOT_FOUND, "session id is not registered")
            }
            Self::Rendezvous(RendezvousError::TokenMismatch) => {
                (StatusCode::FORBIDDEN, "registration token does not match")
            }
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "too many requests"),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rendezvous::EndpointDescriptor;
    use tower::ServiceExt;

    fn test_app() -> Router {
        app(AppState::new(ServerConfig {
            max_requests_per_minute: 100,
        }))
    }

    fn request<T: Serialize>(method: &str, uri: &str, body: &T) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))))
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    fn endpoint(id: &str) -> EndpointDescriptor {
        EndpointDescriptor {
            endpoint_id: id.into(),
            relay_url: Some("https://relay.example.test".into()),
            direct_addrs: vec![],
        }
    }

    #[tokio::test]
    async fn register_and_lookup() {
        let app = test_app();
        let register = RegisterRequest {
            session_id: SessionId::new(123).unwrap(),
            endpoint: endpoint("host-a"),
            ttl_secs: Some(30),
        };

        let response = app
            .clone()
            .oneshot(request("POST", "/v1/register", &register))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/123")
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_token_is_forbidden() {
        let app = test_app();
        let session_id = SessionId::new(456).unwrap();
        app.clone()
            .oneshot(request(
                "POST",
                "/v1/register",
                &RegisterRequest {
                    session_id,
                    endpoint: endpoint("host-a"),
                    ttl_secs: Some(30),
                },
            ))
            .await
            .unwrap();

        let response = app
            .oneshot(request(
                "POST",
                "/v1/unregister",
                &UnregisterRequest {
                    session_id,
                    token: "wrong".into(),
                },
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn rate_limit_returns_429() {
        let app = app(AppState::new(ServerConfig {
            max_requests_per_minute: 1,
        }));
        let session_id = SessionId::new(789).unwrap();
        let first = RegisterRequest {
            session_id,
            endpoint: endpoint("host-a"),
            ttl_secs: Some(30),
        };
        let second = RegisterRequest {
            session_id: SessionId::new(790).unwrap(),
            endpoint: endpoint("host-b"),
            ttl_secs: Some(30),
        };

        assert_eq!(
            app.clone()
                .oneshot(request("POST", "/v1/register", &first))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(request("POST", "/v1/register", &second))
                .await
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
