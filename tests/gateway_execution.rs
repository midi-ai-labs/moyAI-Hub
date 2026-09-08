use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
#[cfg(feature = "tauri-desktop")]
use moyai_hub::gateway_process::{GatewayProcess, acknowledge_quiescent, uncertain};
use moyai_hub::{
    catalog::*,
    core::HubCore,
    gateway::Gateway,
    gateway_process::GatewayControl,
    protocol::Availability,
    service::{SharedCore, now_ms, router_with_gateway},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{Notify, mpsc};

const BOOTSTRAP: &str = "bootstrap-test-secret-000000000000000000000";
const CONTROL: &str = "companion-test-secret-000000000000000000000";
#[derive(Clone)]
struct Provider {
    records: Arc<Mutex<Vec<Value>>>,
    release: Arc<Notify>,
    held: bool,
    truncated: bool,
    reject: bool,
}
struct Stream(mpsc::Receiver<Result<Bytes, io::Error>>);
impl futures_core::Stream for Stream {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
async fn generation(
    State(provider): State<Provider>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    provider
        .records
        .lock()
        .unwrap()
        .push(json!({"body":body,"authorization_present":headers.contains_key("authorization")}));
    if provider.reject {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"INTERNAL_PROVIDER_CANARY http://private-provider/secret"})),
        )
            .into_response();
    }
    let (send, receive) = mpsc::channel(4);
    tokio::spawn(async move {
        let _ = send
            .send(Ok(Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"MODEL_OK\"}}]}\n\n",
            )))
            .await;
        if provider.held {
            provider.release.notified().await;
        }
        if !provider.truncated {
            let _ = send.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
        }
    });
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(Stream(receive)),
    )
        .into_response()
}
struct Fixture {
    core: SharedCore,
    hub: String,
    provider_url: String,
    provider: Provider,
    control: GatewayControl,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    client: reqwest::Client,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.provider.release.notify_one();
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Fixture {
    async fn new(held: bool, truncated: bool) -> Self {
        Self::with_rejection(held, truncated, false).await
    }
    async fn with_rejection(held: bool, truncated: bool, reject: bool) -> Self {
        let provider = Provider {
            records: Arc::new(Mutex::new(Vec::new())),
            release: Arc::new(Notify::new()),
            held,
            truncated,
            reject,
        };
        let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider_url = format!("http://{}/v1", provider_listener.local_addr().unwrap());
        let provider_router = Router::new()
            .route("/v1/chat/completions", post(generation))
            .with_state(provider.clone());
        let provider_task = tokio::spawn(async move {
            axum::serve(provider_listener, provider_router)
                .await
                .unwrap();
        });
        let mut store = CatalogStore::empty("0.1.0");
        let catalog = Catalog {
            models: vec![LogicalModel {
                id: "logical-qwen".into(),
                label: "Selected Qwen".into(),
                capabilities: BTreeSet::from(["tools".into()]),
            }],
            deployments: vec![Deployment {
                id: "deployment-qwen".into(),
                logical_model_id: "logical-qwen".into(),
                capacity_pool_id: "gpu".into(),
                label: "Qwen".into(),
                endpoint: provider_url.clone(),
                provider_model: "actual-provider-model".into(),
                provider_profile: "openai_compatible_chat".into(),
                enabled: true,
            }],
            capacity_pools: vec![CapacityPool {
                id: "gpu".into(),
                label: "GPU".into(),
                max_concurrent: 1,
            }],
            policy: CatalogPolicy {
                global_max_concurrent: 1,
                permit_ttl_ms: 1000,
                heartbeat_stale_ms: 30000,
                lease_ttl_ms: 15000,
            },
        };
        store.replace_catalog(catalog, now_ms()).unwrap();
        let mut owned = HubCore::new(store).unwrap();
        owned
            .observe_present_deployment("deployment-qwen", Availability::Unknown, now_ms())
            .unwrap();
        let core = Arc::new(Mutex::new(owned));
        let hub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub = format!("http://{}", hub_listener.local_addr().unwrap());
        let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_base = format!("http://{}/", gateway_listener.local_addr().unwrap());
        let gateway = Gateway::new(hub.clone(), CONTROL.into()).unwrap();
        let gateway_task = tokio::spawn(async move {
            axum::serve(gateway_listener, gateway.router())
                .await
                .unwrap();
        });
        let control = GatewayControl::for_loopback(gateway_base, CONTROL.into()).unwrap();
        let router = router_with_gateway(core.clone(), BOOTSTRAP.into(), Some(control.clone()));
        let hub_task = tokio::spawn(async move {
            axum::serve(
                hub_listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            core,
            hub,
            provider_url,
            provider,
            control,
            tasks: vec![provider_task, gateway_task, hub_task],
            client: reqwest::Client::new(),
        }
    }
    async fn post(&self, path: &str, token: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.hub))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
    async fn register(&self) -> Value {
        let (status, client) = self
            .post(
                "/v1/clients/register",
                BOOTSTRAP,
                json!({"label":"Desktop"}),
            )
            .await;
        assert_eq!(status, 200);
        for context in ["main", "side_chat"] {
            assert_eq!(self.post("/v1/clients/review",client["client_token"].as_str().unwrap(),json!({"id":client["id"],"context":context,"expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"],
                "selection":{"allowed_model_ids":["logical-qwen"],"preferred_model_id":"logical-qwen","required_capabilities":["tools"],"wait_policy":"wait_for_preferred","affinity_turns":2}})).await.0,200);
        }
        client
    }
    async fn prepare(
        &self,
        client: &Value,
        context: &str,
        turn: &str,
        request: &str,
    ) -> (u16, Value) {
        self.post("/v1/requests/prepare",client["client_token"].as_str().unwrap(),json!({"id":client["id"],"context":context,"turn_id":turn,"request_id":request,"expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"]})).await
    }
    async fn generate(&self, ready: &Value) -> reqwest::Response {
        self.client.post(format!("{}/chat/completions",ready["gateway_base_url"].as_str().unwrap())).bearer_auth(ready["request_token"].as_str().unwrap())
            .json(&json!({"model":ready["logical_model_id"],"messages":[{"role":"user","content":"PROMPT_CANARY"}],"stream":true})).send().await.unwrap()
    }
    async fn wait_status(&self, accept: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let value = self.control.status().await.unwrap();
                assert_eq!(
                    value["running"].as_u64().unwrap(),
                    value["responding"].as_u64().unwrap() + value["draining"].as_u64().unwrap()
                );
                if accept(&value) {
                    return value;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn network_model_permit_requires_exact_client_certificate_and_rechecks_revocation_before_upstream()
 {
    use moyai_hub::{
        network::{
            DeviceNetwork,
            tls::{self, NetworkListener, TlsPeer},
        },
        service::{network_router, router_with_registered_gateway},
    };
    let mut fixture = Fixture::new(false, false).await;
    let directory = tempfile::tempdir().unwrap();
    let hub_id = fixture.core.lock().unwrap().catalog_store().hub_id.clone();
    let mut owner = DeviceNetwork::open(&directory.path().join("network.json"), &hub_id).unwrap();
    let ip = std::net::IpAddr::from([127, 0, 0, 1]);
    fn enroll(
        owner: &mut DeviceNetwork,
        label: &str,
        ip: std::net::IpAddr,
    ) -> (reqwest::Client, String) {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec![ip.to_string()]).unwrap();
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();
        let invite = owner
            .invite(&owner.revision(), label.into(), BTreeSet::new(), now_ms())
            .unwrap();
        let device = owner
            .enroll(invite["code"].as_str().unwrap(), &csr, ip, now_ms())
            .unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .tls_built_in_root_certs(false)
            .add_root_certificate(
                reqwest::Certificate::from_pem(owner.ca_certificate().as_bytes()).unwrap(),
            )
            .identity(
                reqwest::Identity::from_pem(
                    format!("{}\n{}", device.certificate_pem, key.serialize_pem()).as_bytes(),
                )
                .unwrap(),
            )
            .build()
            .unwrap();
        (client, device.device_id)
    }
    let (a, a_id) = enroll(&mut owner, "A", ip);
    let (b, _) = enroll(&mut owner, "B", ip);
    let (certificate, key) = owner.server_certificate(ip, now_ms()).unwrap();
    let ca = owner.ca_certificate().to_owned();
    let network = Arc::new(Mutex::new(owner));
    let internal = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let internal_url = format!("http://{}", internal.local_addr().unwrap());
    let private = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private_url = format!("http://{}", private.local_addr().unwrap());
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_url = format!("https://{}", gateway_listener.local_addr().unwrap());
    let gateway = Gateway::new(internal_url, CONTROL.into()).unwrap();
    let router = gateway.clone().router();
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(private, router).await.unwrap()
    }));
    let listener = NetworkListener::new(
        gateway_listener,
        tls::acceptor(&certificate, &key, &ca, false).unwrap(),
    );
    let router = gateway.network_router();
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<TlsPeer>(),
        )
        .await
        .unwrap()
    }));
    let control = GatewayControl::for_loopback(private_url, CONTROL.into())
        .unwrap()
        .with_network_base_url(gateway_url.clone())
        .unwrap();
    let router = router_with_registered_gateway(
        fixture.core.clone(),
        BOOTSTRAP.into(),
        control.clone(),
        network.clone(),
    );
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(
            internal,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    }));
    let public = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub = format!("https://{}", public.local_addr().unwrap());
    let listener = NetworkListener::new(
        public,
        tls::acceptor(&certificate, &key, &ca, true).unwrap(),
    );
    let router = network_router(fixture.core.clone(), network.clone(), Some(control.clone()));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<TlsPeer>(),
        )
        .await
        .unwrap()
    }));
    let session: Value = a
        .post(format!("{hub}/v1/network/model-session"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        session["default_selection"]["allowed_model_ids"],
        json!(["logical-qwen"])
    );
    assert!(!session.to_string().contains(&fixture.provider_url));
    let token = session["client_token"].as_str().unwrap();
    assert_eq!(a.post(format!("{hub}/v1/clients/review")).bearer_auth(token).json(&json!({"id":session["id"],"context":"main","expected_hub_id":session["hub_id"],"reviewed_revision":session["revision"],"selection":session["default_selection"]})).send().await.unwrap().status(),200);
    let prepare = |request: &str| json!({"id":session["id"],"context":"main","turn_id":"turn","request_id":request,"expected_hub_id":session["hub_id"],"reviewed_revision":session["revision"]});
    let response = a
        .post(format!("{hub}/v1/requests/prepare"))
        .bearer_auth(token)
        .json(&prepare("one"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let ready: Value = response.json().await.unwrap();
    assert_eq!(ready["state"], "ready");
    assert!(
        ready["gateway_base_url"]
            .as_str()
            .unwrap()
            .starts_with(&gateway_url)
    );
    let url = format!(
        "{}/chat/completions",
        ready["gateway_base_url"].as_str().unwrap()
    );
    let body = json!({"model":"logical-qwen","messages":[{"role":"user","content":"NETWORK_PROMPT_CANARY"}],"stream":true});
    assert_eq!(
        b.post(&url)
            .bearer_auth(ready["request_token"].as_str().unwrap())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(fixture.provider.records.lock().unwrap().is_empty());
    let response = a
        .post(&url)
        .bearer_auth(ready["request_token"].as_str().unwrap())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("MODEL_OK"));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if control.status().await.unwrap()["running"] == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before_stop: Value = a
        .post(format!("{hub}/v1/requests/prepare"))
        .bearer_auth(token)
        .json(&prepare("before-stop"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before_stop["state"], "ready");
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner.set_admission(&revision, &a_id, true).unwrap();
    }
    let stopped = a
        .post(format!("{hub}/v1/requests/prepare"))
        .bearer_auth(token)
        .json(&prepare("while-stopped"))
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), 403);
    assert_eq!(
        stopped.json::<Value>().await.unwrap()["error"],
        "device_stopped"
    );
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner.set_admission(&revision, &a_id, false).unwrap();
    }
    // This ticket was never used while stopped. Re-enabling must still not revive it.
    let retired = a
        .post(format!(
            "{}/chat/completions",
            before_stop["gateway_base_url"].as_str().unwrap()
        ))
        .bearer_auth(before_stop["request_token"].as_str().unwrap())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(retired.status(), 403);
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if control.status().await.unwrap()["running"] == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ready: Value = a
        .post(format!("{hub}/v1/requests/prepare"))
        .bearer_auth(token)
        .json(&prepare("two"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ready["state"], "ready");
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner
            .set_device(&revision, &a_id, "A".into(), BTreeSet::new(), true)
            .unwrap();
    }
    let response = a
        .post(format!(
            "{}/chat/completions",
            ready["gateway_base_url"].as_str().unwrap()
        ))
        .bearer_auth(ready["request_token"].as_str().unwrap())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.provider.records.lock().unwrap()[0]["body"]["model"],
        "actual-provider-model"
    );
    assert!(
        !fixture.provider.records.lock().unwrap()[0]["authorization_present"]
            .as_bool()
            .unwrap()
    );
}

#[tokio::test]
async fn public_prepare_is_idempotent_and_one_claim_rewrites_only_provider_model() {
    let fixture = Fixture::new(false, false).await;
    let client = fixture.register().await;
    let (status, ready) = fixture
        .prepare(&client, "main", "turn-a", "request-a")
        .await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["provider_profile"], "openai_compatible_chat");
    assert_eq!(ready["provider_api_mode"], "chat_completions");
    assert!(!ready.to_string().contains(&fixture.provider_url));
    assert!(!ready.to_string().contains("actual-provider-model"));
    assert_eq!(
        fixture
            .prepare(&client, "main", "turn-a", "request-a")
            .await
            .1,
        ready
    );
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status().as_u16(), 200);
    assert!(response.text().await.unwrap().contains("MODEL_OK"));
    fixture.wait_status(|value| value["running"] == 0).await;
    assert_eq!(fixture.generate(&ready).await.status().as_u16(), 409);
    let records = fixture.provider.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["body"]["model"], "actual-provider-model");
    assert_eq!(
        records[0]["body"]["messages"][0]["content"],
        "PROMPT_CANARY"
    );
    assert_eq!(records[0]["authorization_present"], false);
}

#[tokio::test]
async fn provider_error_body_is_redacted_while_confirmed_error_releases_capacity() {
    let fixture = Fixture::with_rejection(false, false, true).await;
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"provider_rejected"})
    );
    let status = fixture.wait_status(|value| value["running"] == 0).await;
    assert_eq!(status["quarantined_pools"], json!([]));
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn downstream_stop_and_advisory_expiry_do_not_release_running_gateway_capacity() {
    let fixture = Fixture::new(true, false).await;
    let client = fixture.register().await;
    let ready = fixture
        .prepare(&client, "main", "turn-main", "request-main")
        .await
        .1;
    let mut response = fixture.generate(&ready).await;
    assert!(response.chunk().await.unwrap().is_some());
    let status = fixture.control.status().await.unwrap();
    assert_eq!(status["responding"], 1);
    assert_eq!(status["draining"], 0);
    drop(response);
    assert_eq!(
        fixture
            .post(
                "/v1/turns/cancel",
                client["client_token"].as_str().unwrap(),
                json!({"id":client["id"],"context":"main","turn_id":"turn-main"})
            )
            .await
            .0,
        200
    );
    tokio::time::sleep(Duration::from_millis(1050)).await;
    let status = fixture.wait_status(|value| value["draining"] == 1).await;
    assert_eq!(status["running"], 1);
    assert_eq!(status["responding"], 0);
    let waiting = fixture
        .prepare(&client, "side_chat", "turn-side", "request-side")
        .await
        .1;
    assert_eq!(waiting["state"], "waiting");
    assert_eq!(waiting["reason"], "global_capacity");
    // This request has never been admitted. Waiting longer than its provisional
    // permit TTL must keep waiting with the same request ID, not consume it.
    tokio::time::sleep(Duration::from_millis(1050)).await;
    let (status, still_waiting) = fixture
        .prepare(&client, "side_chat", "turn-side", "request-side")
        .await;
    assert_eq!(status, 200);
    assert_eq!(still_waiting["state"], "waiting");
    fixture.provider.release.notify_one();
    let status = fixture.wait_status(|value| value["running"] == 0).await;
    assert_eq!(status["responding"], 0);
    assert_eq!(status["draining"], 0);
    let (first, concurrent) = tokio::join!(
        fixture.prepare(&client, "side_chat", "turn-side", "request-side"),
        fixture.prepare(&client, "side_chat", "turn-side", "request-side")
    );
    assert_eq!(first, concurrent);
    let ready = first.1;
    assert_eq!(ready["state"], "ready");
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
    assert_eq!(
        fixture
            .post(
                "/v1/turns/finish",
                client["client_token"].as_str().unwrap(),
                json!({"id":client["id"],"context":"side_chat","turn_id":"turn-side"})
            )
            .await
            .0,
        200
    );
    assert_eq!(fixture.control.status().await.unwrap()["reserved"], 0);
}

#[tokio::test]
async fn receiver_close_racing_upstream_terminal_counts_the_execution_once() {
    let fixture = Fixture::new(true, false).await;
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let mut response = fixture.generate(&ready).await;
    assert!(response.chunk().await.unwrap().is_some());
    fixture.provider.release.notify_one();
    drop(response);
    let status = fixture.wait_status(|value| value["running"] == 0).await;
    assert_eq!(status["responding"], 0);
    assert_eq!(status["draining"], 0);
    assert_eq!(status["quarantined_pools"], json!([]));
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn revision_and_identity_are_rechecked_before_claim_without_upstream_side_effects() {
    let fixture = Fixture::new(false, false).await;
    let client = fixture.register().await;
    let other = fixture.register().await;
    assert_eq!(fixture.post("/v1/requests/prepare",other["client_token"].as_str().unwrap(),json!({"id":client["id"],"context":"main","turn_id":"turn","request_id":"request","expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"]})).await.0,401);
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    {
        let mut core = fixture.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        catalog.models[0].label = "Revised".into();
        core.replace_catalog(catalog, now_ms()).unwrap();
    }
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status().as_u16(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "review_required"
    );
    assert_eq!(
        fixture
            .prepare(&client, "side_chat", "other-turn", "other-request")
            .await
            .0,
        409
    );
    assert!(fixture.provider.records.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .client
            .post(format!("{}/_gateway/claim", fixture.hub))
            .bearer_auth(BOOTSTRAP)
            .json(&json!({"permit_id":ready["permit_id"]}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
}

#[tokio::test]
async fn truncated_upstream_quarantines_pool_instead_of_freeing_it_for_reuse() {
    let fixture = Fixture::new(false, true).await;
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let _ = fixture.generate(&ready).await.text().await;
    let status = fixture
        .wait_status(|value| {
            value["quarantined_pools"]
                .as_array()
                .is_some_and(|pools| !pools.is_empty())
        })
        .await;
    assert_eq!(status["quarantined_pools"], json!(["gpu"]));
    fixture
        .post(
            "/v1/turns/finish",
            client["client_token"].as_str().unwrap(),
            json!({"id":client["id"],"context":"main","turn_id":"turn"}),
        )
        .await;
    let (status, error) = fixture
        .prepare(&client, "side_chat", "next-turn", "next-request")
        .await;
    assert_eq!(status, 503);
    assert_eq!(error["error"], "gateway_quarantined");
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unclaimed_ticket_expires_and_wrong_model_or_credential_never_contacts_provider() {
    let fixture = Fixture::new(false, false).await;
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let target = format!(
        "{}/chat/completions",
        ready["gateway_base_url"].as_str().unwrap()
    );
    let body = json!({"model":"wrong-model","messages":[],"stream":true});
    assert_eq!(
        fixture
            .client
            .post(&target)
            .bearer_auth("wrong-token")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    assert_eq!(
        fixture
            .client
            .post(&target)
            .bearer_auth(ready["request_token"].as_str().unwrap())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        400
    );
    tokio::time::sleep(Duration::from_millis(1050)).await;
    assert_eq!(fixture.generate(&ready).await.status().as_u16(), 409);
    assert_eq!(fixture.control.status().await.unwrap()["reserved"], 0);
    assert!(fixture.provider.records.lock().unwrap().is_empty());
}

#[tokio::test]
#[cfg(feature = "tauri-desktop")]
async fn companion_clean_stop_clears_marker_but_lost_owner_needs_explicit_quiescence_ack() {
    let directory = tempfile::tempdir().unwrap();
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_moyai-hub"));
    let process = GatewayProcess::start(executable, "http://127.0.0.1:9".into(), directory.path())
        .await
        .unwrap();
    assert!(uncertain(directory.path()));
    assert!(acknowledge_quiescent(directory.path()).is_err());
    assert!(process.stop().await);
    assert!(!uncertain(directory.path()));
    let process = GatewayProcess::start(executable, "http://127.0.0.1:9".into(), directory.path())
        .await
        .unwrap();
    drop(process);
    assert!(uncertain(directory.path()));
    assert!(
        GatewayProcess::start(executable, "http://127.0.0.1:9".into(), directory.path())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if acknowledge_quiescent(directory.path()).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(!uncertain(directory.path()));
}

#[tokio::test]
#[cfg(feature = "tauri-desktop")]
async fn fixed_gateway_port_conflict_is_clean_before_admission_and_can_retry_exact_port() {
    use moyai_hub::{gateway_process::GatewayNetwork, network::DeviceNetwork};
    let directory = tempfile::tempdir().unwrap();
    let owner =
        DeviceNetwork::open(&directory.path().join("network.json"), "startup-test").unwrap();
    let ip = "127.0.0.1".parse().unwrap();
    let (cert, key) = owner.server_certificate(ip, now_ms()).unwrap();
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = occupied.local_addr().unwrap();
    let network = || GatewayNetwork {
        bind: address,
        advertise_ip: ip,
        certificate_pem: cert.clone(),
        private_key_pem: key.clone(),
        ca_certificate_pem: owner.ca_certificate().to_owned(),
    };
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_moyai-hub"));
    let result = GatewayProcess::start_with_network(
        executable,
        "http://127.0.0.1:9".into(),
        directory.path(),
        Some(network()),
    )
    .await;
    assert!(result.is_err());
    assert!(
        !uncertain(directory.path()),
        "known pre-admission bind failure must not claim a provider may be running"
    );
    drop(occupied);
    let process = GatewayProcess::start_with_network(
        executable,
        "http://127.0.0.1:9".into(),
        directory.path(),
        Some(network()),
    )
    .await
    .unwrap();
    assert_eq!(
        process.control.network_base_url(),
        Some(format!("https://{address}").as_str())
    );
    assert!(process.stop().await);
    assert!(!uncertain(directory.path()));
}

#[tokio::test]
#[cfg(feature = "tauri-desktop")]
async fn unclassified_gateway_startup_failure_preserves_uncertainty() {
    use moyai_hub::{gateway_process::GatewayNetwork, network::DeviceNetwork};
    let directory = tempfile::tempdir().unwrap();
    let owner =
        DeviceNetwork::open(&directory.path().join("network.json"), "startup-error-test").unwrap();
    let ip = "127.0.0.1".parse().unwrap();
    let (cert, _) = owner.server_certificate(ip, now_ms()).unwrap();
    let result = GatewayProcess::start_with_network(
        std::path::Path::new(env!("CARGO_BIN_EXE_moyai-hub")),
        "http://127.0.0.1:9".into(),
        directory.path(),
        Some(GatewayNetwork {
            bind: "127.0.0.1:0".parse().unwrap(),
            advertise_ip: ip,
            certificate_pem: cert,
            private_key_pem: "invalid-key".into(),
            ca_certificate_pem: owner.ca_certificate().to_owned(),
        }),
    )
    .await;
    assert!(result.is_err());
    assert!(uncertain(directory.path()));
    tokio::time::timeout(Duration::from_secs(4), async {
        while acknowledge_quiescent(directory.path()).is_err() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}
