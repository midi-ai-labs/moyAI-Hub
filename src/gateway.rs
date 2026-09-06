//! Generation traffic lives in the `--gateway` companion process, never the Hub router.
//! Reservations expire; claimed requests hold capacity until upstream terminal settlement.
use crate::{catalog::Deployment, protocol::ExecutionPermit, service::now_ms};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{Notify, mpsc, oneshot};

const REQUEST_LIMIT: usize = 8 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;
const UPSTREAM_INACTIVITY: Duration = Duration::from_secs(300);
const DRAIN_DEADLINE: Duration = Duration::from_secs(305);

fn upstream_client(inactivity: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        // Headers and each response read have an inactivity deadline. Progressing
        // generation has no total-duration deadline.
        .read_timeout(inactivity)
        .build()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub permit: ExecutionPermit,
    pub deployment: Deployment,
    pub pool_limit: u32,
    pub global_limit: u32,
    #[serde(default)]
    pub device_certificate_sha256: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reserved,
    Running,
    Settled,
    Uncertain,
}
struct Record {
    grant: Grant,
    token: String,
    phase: Phase,
    // Observe the existing response channel without keeping the stream open or
    // independently owning a second execution/stop state.
    downstream: Option<mpsc::WeakSender<Result<Bytes, io::Error>>>,
}
struct Registry {
    grants: BTreeMap<String, Record>,
    quarantined: BTreeSet<String>,
    accepting: bool,
}

pub struct Gateway {
    registry: Mutex<Registry>,
    control_token: String,
    hub_url: String,
    client: reqwest::Client,
    changed: Notify,
}

type ApiError = (StatusCode, Json<Value>);
fn error(status: StatusCode, code: &str) -> ApiError {
    (status, Json(json!({"error":code})))
}
fn secret_matches(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left.len() <= 256
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |v, (a, b)| v | (a ^ b))
            == 0
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    !headers.contains_key("origin")
        && headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| secret_matches(value, token))
}
fn token() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}

impl Gateway {
    pub fn new(hub_url: String, control_token: String) -> Result<Arc<Self>, reqwest::Error> {
        Ok(Arc::new(Self {
            registry: Mutex::new(Registry {
                grants: BTreeMap::new(),
                quarantined: BTreeSet::new(),
                accepting: true,
            }),
            control_token,
            hub_url,
            client: upstream_client(UPSTREAM_INACTIVITY)?,
            changed: Notify::new(),
        }))
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/control/grants", post(admit))
            .route("/control/cancel", post(cancel))
            .route("/control/state", get(status))
            .route("/r/{permit_id}/v1/chat/completions", post(generate))
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .with_state(self)
    }
    pub fn network_router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/r/{permit_id}/v1/chat/completions", post(generate))
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .with_state(self)
    }
    pub fn status(&self) -> Value {
        let mut registry = self.registry.lock().unwrap();
        registry.expire(now_ms());
        let mut responding = 0;
        let mut draining = 0;
        for record in registry
            .grants
            .values()
            .filter(|record| record.phase == Phase::Running)
        {
            if record
                .downstream
                .as_ref()
                .and_then(mpsc::WeakSender::upgrade)
                .is_some_and(|sender| !sender.is_closed())
            {
                responding += 1;
            } else {
                draining += 1;
            }
        }
        json!({"accepting":registry.accepting,"reserved":registry.grants.values().filter(|r|r.phase==Phase::Reserved).count(),
            "running":responding + draining,"responding":responding,"draining":draining,"quarantined_pools":registry.quarantined,
            "scope":"gateway_requests_only"})
    }
    pub async fn drain(&self) -> bool {
        {
            let mut registry = self.registry.lock().unwrap();
            registry.accepting = false;
            for record in registry.grants.values_mut() {
                if record.phase == Phase::Reserved {
                    record.phase = Phase::Settled;
                }
            }
        }
        let wait = async {
            loop {
                let notified = self.changed.notified();
                if self
                    .registry
                    .lock()
                    .unwrap()
                    .grants
                    .values()
                    .all(|record| record.phase != Phase::Running)
                {
                    break;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(DRAIN_DEADLINE, wait).await.is_err() {
            return false;
        }
        self.registry.lock().unwrap().quarantined.is_empty()
    }
    async fn settled(&self, permit_id: &str, uncertain: bool) {
        {
            let mut registry = self.registry.lock().unwrap();
            let pool = registry.grants.get_mut(permit_id).map(|record| {
                record.phase = if uncertain {
                    Phase::Uncertain
                } else {
                    Phase::Settled
                };
                record.grant.deployment.capacity_pool_id.clone()
            });
            if uncertain {
                if let Some(pool) = pool {
                    registry.quarantined.insert(pool);
                }
            }
        }
        self.changed.notify_waiters();
        // A failed metadata callback cannot release this process's capacity early.
        let _ = self
            .client
            .post(format!("{}/_gateway/settle", self.hub_url))
            .bearer_auth(&self.control_token)
            .json(&json!({"permit_id":permit_id}))
            .timeout(Duration::from_secs(3))
            .send()
            .await;
    }
}
impl Registry {
    fn expire(&mut self, now: u64) {
        for record in self.grants.values_mut() {
            if record.phase == Phase::Reserved && record.grant.permit.expires_at_ms <= now {
                record.phase = Phase::Settled;
            }
        }
        self.grants.retain(|_, record| {
            record.phase != Phase::Settled
                || now < record.grant.permit.expires_at_ms.saturating_add(60_000)
        });
    }
}

async fn admit(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
    Json(grant): Json<Grant>,
) -> Result<Json<Value>, ApiError> {
    if !authorized(&headers, &gateway.control_token) {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    if grant.permit.deployment_id != grant.deployment.id
        || grant.permit.logical_model_id != grant.deployment.logical_model_id
        || grant.pool_limit == 0
        || grant.global_limit == 0
        || grant.permit.expires_at_ms <= now_ms()
        || !matches!(
            grant.deployment.provider_profile.as_str(),
            "openai_compatible_chat" | "lmstudio_chat"
        )
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_grant"));
    }
    let mut registry = gateway.registry.lock().unwrap();
    registry.expire(now_ms());
    if !registry.accepting {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_unavailable",
        ));
    }
    if registry
        .quarantined
        .contains(&grant.deployment.capacity_pool_id)
    {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_quarantined",
        ));
    }
    if let Some(existing) = registry.grants.get(&grant.permit.id) {
        if existing.phase != Phase::Reserved {
            return Err(error(StatusCode::CONFLICT, "permit_consumed"));
        }
        return Ok(Json(
            json!({"state":"ready","request_token":existing.token}),
        ));
    }
    let active: Vec<_> = registry
        .grants
        .values()
        .filter(|record| matches!(record.phase, Phase::Reserved | Phase::Running))
        .collect();
    if active.len() >= grant.global_limit as usize {
        return Ok(Json(
            json!({"state":"waiting","reason":"global_capacity","retry_after_ms":1000}),
        ));
    }
    if active
        .iter()
        .filter(|record| {
            record.grant.deployment.capacity_pool_id == grant.deployment.capacity_pool_id
        })
        .count()
        >= grant.pool_limit as usize
    {
        return Ok(Json(
            json!({"state":"waiting","reason":"busy","retry_after_ms":1000}),
        ));
    }
    if registry.grants.len() >= 1024 {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "registry_full"));
    }
    let secret = token();
    registry.grants.insert(
        grant.permit.id.clone(),
        Record {
            grant,
            token: secret.clone(),
            phase: Phase::Reserved,
            downstream: None,
        },
    );
    Ok(Json(json!({"state":"ready","request_token":secret})))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelScope {
    pub client_id: String,
    pub context_id: Option<String>,
    pub turn_id: Option<String>,
}
async fn cancel(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
    Json(scope): Json<CancelScope>,
) -> Result<Json<Value>, ApiError> {
    if !authorized(&headers, &gateway.control_token) {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let mut registry = gateway.registry.lock().unwrap();
    for record in registry.grants.values_mut() {
        let permit = &record.grant.permit;
        if permit.client_id == scope.client_id
            && scope
                .context_id
                .as_ref()
                .is_none_or(|id| id == &permit.context_id)
            && scope
                .turn_id
                .as_ref()
                .is_none_or(|id| id == &permit.turn_id)
            && record.phase == Phase::Reserved
        {
            record.phase = Phase::Settled;
        }
    }
    Ok(Json(
        json!({"canceled":true,"running_requests":"drain_until_terminal"}),
    ))
}
async fn status(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    if !authorized(&headers, &gateway.control_token) {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(Json(gateway.status()))
}

struct ChunkStream(mpsc::Receiver<Result<Bytes, io::Error>>);
impl futures_core::Stream for ChunkStream {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
type ResponseHead = Result<(StatusCode, String), ApiError>;
async fn generate(
    State(gateway): State<Arc<Gateway>>,
    Path(permit_id): Path<String>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<crate::network::tls::TlsPeer>>>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let bytes = match body {
        Ok(body) => body,
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large").into_response(),
    };
    let mut body: Value = match serde_json::from_slice(&bytes) {
        Ok(Value::Object(value)) => Value::Object(value),
        _ => return error(StatusCode::BAD_REQUEST, "invalid_request").into_response(),
    };
    let (head_send, head_receive) = oneshot::channel();
    let (send, receive) = mpsc::channel(8);
    let grant = {
        let mut registry = gateway.registry.lock().unwrap();
        registry.expire(now_ms());
        if !registry.accepting {
            return error(StatusCode::SERVICE_UNAVAILABLE, "gateway_unavailable").into_response();
        }
        if registry.grants.get(&permit_id).is_some_and(|record| {
            registry
                .quarantined
                .contains(&record.grant.deployment.capacity_pool_id)
        }) {
            return error(StatusCode::SERVICE_UNAVAILABLE, "gateway_quarantined").into_response();
        }
        let Some(record) = registry.grants.get_mut(&permit_id) else {
            return error(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        };
        if !authorized(&headers, &record.token) {
            return error(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
        match (&record.grant.device_certificate_sha256, &peer) {
            (Some(expected), Some(axum::Extension(axum::extract::ConnectInfo(peer))))
                if peer.certificate_sha256.as_ref() == Some(expected)
                    && crate::network::tls::authority_matches(&headers, &peer.authority) => {}
            (None, None) => {}
            _ => return error(StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        }
        if record.phase != Phase::Reserved {
            return error(StatusCode::CONFLICT, "permit_consumed").into_response();
        }
        if body.get("model").and_then(Value::as_str) != Some(&record.grant.permit.logical_model_id)
            || !body.get("messages").is_some_and(Value::is_array)
        {
            return error(StatusCode::BAD_REQUEST, "invalid_request").into_response();
        }
        record.phase = Phase::Running;
        record.downstream = Some(send.downgrade());
        record.grant.clone()
    };
    body["model"] = json!(grant.deployment.provider_model);
    tokio::spawn(run_request(gateway, grant, body, head_send, send));
    match head_receive.await {
        Ok(Ok((status, content_type))) => (
            status,
            [
                ("content-type", content_type),
                ("cache-control", "no-store".into()),
            ],
            Body::from_stream(ChunkStream(receive)),
        )
            .into_response(),
        Ok(Err(error)) => error.into_response(),
        Err(_) => error(StatusCode::BAD_GATEWAY, "upstream_uncertain").into_response(),
    }
}

fn upstream_url(endpoint: &str) -> Option<String> {
    let url = reqwest::Url::parse(endpoint).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let endpoint = endpoint.trim_end_matches('/');
    Some(if endpoint.ends_with("/v1") {
        format!("{endpoint}/chat/completions")
    } else {
        format!("{endpoint}/v1/chat/completions")
    })
}

#[derive(Default)]
struct SseTerminal {
    line: Vec<u8>,
    done: bool,
}
impl SseTerminal {
    fn feed(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            if byte == b'\n' {
                if self.line.strip_suffix(b"\r").unwrap_or(&self.line) == b"data: [DONE]" {
                    self.done = true;
                }
                self.line.clear();
            } else {
                self.line.push(byte);
                if self.line.len() > 1024 * 1024 {
                    return false;
                }
            }
        }
        true
    }
}

async fn run_request(
    gateway: Arc<Gateway>,
    grant: Grant,
    body: Value,
    head: oneshot::Sender<ResponseHead>,
    send: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    let permit_id = grant.permit.id.clone();
    let operation = async {
        let claim = gateway
            .client
            .post(format!("{}/_gateway/claim", gateway.hub_url))
            .bearer_auth(&gateway.control_token)
            .json(&json!({"permit_id":permit_id,"device_certificate_sha256":grant.device_certificate_sha256}))
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        let claim = match claim {
            Ok(value) => value,
            Err(_) => {
                let _ = head.send(Err(error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "gateway_unavailable",
                )));
                return false;
            }
        };
        if !claim.status().is_success() {
            let status = claim.status();
            let code = claim
                .json::<Value>()
                .await
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or("permit_expired".into());
            let _ = head.send(Err(error(status, &code)));
            return false;
        }
        let Some(url) = upstream_url(&grant.deployment.endpoint) else {
            let _ = head.send(Err(error(StatusCode::BAD_GATEWAY, "invalid_provider")));
            return false;
        };
        let upstream = gateway.client.post(url).json(&body).send().await;
        let mut upstream = match upstream {
            Ok(value) => value,
            Err(failure) => {
                let uncertain = !failure.is_connect();
                let _ = head.send(Err(error(
                    StatusCode::BAD_GATEWAY,
                    if uncertain {
                        "upstream_uncertain"
                    } else {
                        "provider_unavailable"
                    },
                )));
                return uncertain;
            }
        };
        let status = upstream.status();
        let sse =
            body.get("stream").and_then(Value::as_bool).unwrap_or(false) && status.is_success();
        let content_type = if sse {
            "text/event-stream"
        } else {
            "application/json"
        };
        if status.is_success() {
            let _ = head.send(Ok((status, content_type.into())));
        } else {
            // Provider errors can contain internal targets or other diagnostics. Send
            // only a safe code downstream, but still drain and verify the upstream body.
            let _ = head.send(Err(error(status, "provider_rejected")));
        }
        let mut terminal = SseTerminal::default();
        let mut json_body = Vec::new();
        let mut total = 0_usize;
        loop {
            match upstream.chunk().await {
                Ok(Some(bytes)) => {
                    total = total.saturating_add(bytes.len());
                    if total > RESPONSE_LIMIT || (sse && !terminal.feed(&bytes)) {
                        return true;
                    }
                    if !sse {
                        json_body.extend_from_slice(&bytes);
                    }
                    // Receiver disappearance is a client Stop, not provider settlement.
                    if status.is_success() {
                        let _ = send.send(Ok(bytes)).await;
                    }
                }
                Ok(None) => {
                    return if sse {
                        !terminal.done
                    } else {
                        serde_json::from_slice::<Value>(&json_body).is_err()
                    };
                }
                Err(_) => return true,
            }
        }
    };
    let uncertain = operation.await;
    if uncertain {
        let _ = send.try_send(Err(io::Error::other(
            "upstream execution could not be confirmed complete",
        )));
    }
    gateway.settled(&permit_id, uncertain).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_channel_closure_and_weak_expiry_do_not_release_or_double_count_execution() {
        let gateway = Gateway::new("http://127.0.0.1:9".into(), "test".into()).unwrap();
        let (send, receive) = mpsc::channel(1);
        let grant = Grant {
            permit: ExecutionPermit {
                id: "permit".into(),
                client_id: "client".into(),
                context_id: "main".into(),
                lease_id: "lease".into(),
                request_id: "request".into(),
                turn_id: "turn".into(),
                catalog_revision: 1,
                logical_model_id: "model".into(),
                deployment_id: "deployment".into(),
                provider_profile: "openai_compatible_chat".into(),
                expires_at_ms: u64::MAX,
                advisory_only: true,
            },
            deployment: Deployment {
                id: "deployment".into(),
                logical_model_id: "model".into(),
                capacity_pool_id: "pool".into(),
                label: "Model".into(),
                endpoint: "http://127.0.0.1:9".into(),
                provider_model: "model".into(),
                provider_profile: "openai_compatible_chat".into(),
                enabled: true,
            },
            pool_limit: 1,
            global_limit: 1,
            device_certificate_sha256: None,
        };
        gateway.registry.lock().unwrap().grants.insert(
            "permit".into(),
            Record {
                grant,
                token: "test".into(),
                phase: Phase::Running,
                downstream: Some(send.downgrade()),
            },
        );
        let status = gateway.status();
        assert_eq!(
            (
                status["running"].as_u64(),
                status["responding"].as_u64(),
                status["draining"].as_u64()
            ),
            (Some(1), Some(1), Some(0))
        );
        drop(receive);
        let status = gateway.status();
        assert_eq!(
            (
                status["running"].as_u64(),
                status["responding"].as_u64(),
                status["draining"].as_u64()
            ),
            (Some(1), Some(0), Some(1))
        );
        drop(send);
        let status = gateway.status();
        assert_eq!(
            (
                status["running"].as_u64(),
                status["responding"].as_u64(),
                status["draining"].as_u64()
            ),
            (Some(1), Some(0), Some(1))
        );
        gateway
            .registry
            .lock()
            .unwrap()
            .grants
            .get_mut("permit")
            .unwrap()
            .phase = Phase::Settled;
        let status = gateway.status();
        assert_eq!(
            (
                status["running"].as_u64(),
                status["responding"].as_u64(),
                status["draining"].as_u64()
            ),
            (Some(0), Some(0), Some(0))
        );
    }
    #[tokio::test]
    async fn upstream_progress_outlives_inactivity_window_but_silence_expires() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().route(
            "/",
            get(|| async {
                let (send, receive) = mpsc::channel(1);
                tokio::spawn(async move {
                    for _ in 0..6 {
                        if send
                            .send(Ok(Bytes::from_static(b"progress")))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                });
                Body::from_stream(ChunkStream(receive))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let deadline = Duration::from_millis(200);
        let started = std::time::Instant::now();
        let mut response = upstream_client(deadline)
            .unwrap()
            .get(target)
            .send()
            .await
            .unwrap();
        let mut received = 0;
        let error = loop {
            match response.chunk().await {
                Ok(Some(bytes)) => received += bytes.len(),
                Ok(None) => panic!("silent response unexpectedly completed"),
                Err(error) => break error,
            }
        };
        server.abort();
        assert_eq!(received, 6 * b"progress".len());
        assert!(started.elapsed() > deadline);
        assert!(error.is_timeout());
    }
    #[test]
    fn terminal_requires_an_exact_sse_line_and_accepts_split_chunks() {
        let mut terminal = SseTerminal::default();
        assert!(terminal.feed(b"data: {\"content\":\"data: [DONE]\"}\n\n"));
        assert!(!terminal.done);
        terminal.feed(b"data: [DO");
        terminal.feed(b"NE]\r\n\n");
        assert!(terminal.done);
    }
    #[test]
    fn target_normalization_rejects_credential_and_query_handoff() {
        assert_eq!(
            upstream_url("http://127.0.0.1:8119/v1/").unwrap(),
            "http://127.0.0.1:8119/v1/chat/completions"
        );
        assert!(upstream_url("http://secret@localhost:8119").is_none());
        assert!(upstream_url("http://localhost:8119?secret=x").is_none());
    }
}
