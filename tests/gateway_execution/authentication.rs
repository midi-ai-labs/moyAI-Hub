use super::*;
use moyai_hub::upstream_auth;
use std::path::Path;

const KEY: &str = "upstream-test-key-never-a-client-token";
const OTHER_KEY: &str = "upstream-other-test-key";
const PRIVATE_ERROR: &str = "PRIVATE_PROVIDER_AUTH_ERROR";

#[derive(Clone, Default)]
struct AuthProvider {
    metadata: Arc<Mutex<Vec<Option<String>>>>,
    generations: Arc<Mutex<Vec<Value>>>,
    reject: Arc<AtomicBool>,
    hold_response: Arc<AtomicBool>,
    release_response: Arc<Notify>,
}

fn authorization(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":PRIVATE_ERROR,"received_api_key":KEY})),
    )
        .into_response()
}

async fn metadata(State(provider): State<AuthProvider>, headers: HeaderMap) -> Response {
    let received = authorization(&headers);
    provider.metadata.lock().unwrap().push(received.clone());
    if received.as_deref() != Some(format!("Bearer {KEY}").as_str()) {
        return unauthorized();
    }
    // Each profile reads only its own collection from this fixture's response.
    Json(json!({
        "data":[{"id":"actual-provider-model"}],
        "models":[{"key":"actual-provider-model","type":"llm","loaded_instances":[]}]
    }))
    .into_response()
}

async fn generation(
    State(provider): State<AuthProvider>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let received = authorization(&headers);
    provider
        .generations
        .lock()
        .unwrap()
        .push(json!({"authorization":received,"body":body}));
    if received.as_deref() != Some(format!("Bearer {KEY}").as_str())
        || provider.reject.load(Ordering::SeqCst)
    {
        return unauthorized();
    }
    if provider.hold_response.load(Ordering::SeqCst) {
        let (send, receive) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = send
                .send(Ok(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"AUTHENTICATED_MODEL_OK\"}}]}\n\n",
                )))
                .await;
            provider.release_response.notified().await;
            let _ = send.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
        });
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(Stream(receive)),
        )
            .into_response();
    }
    let stream = if body.get("input").is_some() {
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"authenticated-response\",\"status\":\"completed\",\"output\":[]}}\n\n"
    } else {
        "data: {\"choices\":[{\"delta\":{\"content\":\"AUTHENTICATED_MODEL_OK\"}}]}\n\ndata: [DONE]\n\n"
    };
    ([("content-type", "text/event-stream")], stream).into_response()
}

async fn fixture() -> (Fixture, AuthProvider) {
    let mut fixture = Fixture::new(false, false).await;
    let provider = AuthProvider::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture.provider_url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/v1/models", get(metadata))
        .route("/api/v1/models", get(metadata))
        .route("/v1/chat/completions", post(generation))
        .route("/v1/responses", post(generation))
        .with_state(provider.clone());
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    (fixture, provider)
}

fn configure(fixture: &Fixture, profile: &str, credential_id: String, stale: bool) {
    let mut core = fixture.core.lock().unwrap();
    let mut catalog = core.catalog_store().catalog.clone();
    let deployment = &mut catalog.deployments[0];
    deployment.endpoint = fixture.provider_url.clone();
    deployment.provider_profile = profile.into();
    deployment.upstream_credential_id = Some(credential_id);
    core.replace_catalog(catalog, now_ms()).unwrap();
    if stale {
        core.observe_deployment("deployment-qwen", Availability::Unknown, 0)
            .unwrap();
    } else {
        core.observe_present_deployment("deployment-qwen", Availability::Unknown, now_ms())
            .unwrap();
    }
}

fn store(directory: &Path, endpoint: &str, profile: &str, key: &str) -> String {
    upstream_auth::store(directory, endpoint, profile, key).unwrap()
}

fn assert_private(value: &Value, credential_id: &str, endpoint: &str) {
    let text = value.to_string();
    for secret in [KEY, OTHER_KEY, PRIVATE_ERROR, credential_id, endpoint] {
        assert!(!text.contains(secret), "public response leaked {secret}");
    }
}

#[tokio::test]
async fn companion_uses_only_hub_credential_for_presence_and_both_generation_apis() {
    for profile in [
        "openai_compatible_chat",
        "openai_compatible_responses",
        "lmstudio_chat",
        "lmstudio_responses",
    ] {
        let (mut fixture, provider) = fixture().await;
        let directory = tempfile::tempdir().unwrap();
        let credential_id = store(directory.path(), &fixture.provider_url, profile, KEY);
        configure(&fixture, profile, credential_id.clone(), true);
        let process = fixture.companion(directory.path()).await;
        let client = fixture.register().await;
        let catalog: Value = fixture
            .client
            .get(format!("{}/v1/catalog", fixture.hub))
            .bearer_auth(client["client_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_private(&catalog, &credential_id, &fixture.provider_url);
        let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
        assert_eq!(status, 200, "{ready}");
        assert_eq!(ready["state"], "ready", "{profile}: {ready}");
        assert_private(&ready, &credential_id, &fixture.provider_url);
        let response = if profile.ends_with("responses") {
            fixture
                .client
                .post(format!("{}/responses", ready["gateway_base_url"].as_str().unwrap()))
                .bearer_auth(ready["request_token"].as_str().unwrap())
                .json(&json!({"model":ready["logical_model_id"],"instructions":"Answer the user.","input":[{"role":"user","content":"PROMPT_CANARY"}],"store":false,"stream":true}))
                .send()
                .await
                .unwrap()
        } else {
            fixture.generate(&ready).await
        };
        assert_eq!(response.status(), StatusCode::OK, "{profile}");
        let body = response.text().await.unwrap();
        assert!(body.contains("AUTHENTICATED_MODEL_OK") || body.contains("response.completed"));
        fixture.wait_status(|state| state["running"] == 0).await;
        assert_eq!(
            *provider.metadata.lock().unwrap(),
            vec![Some(format!("Bearer {KEY}"))],
            "a stale deployment must refresh using its own Hub credential"
        );
        {
            let generations = provider.generations.lock().unwrap();
            assert_eq!(generations.len(), 1);
            assert_eq!(generations[0]["authorization"], format!("Bearer {KEY}"));
            assert_eq!(generations[0]["body"]["model"], "actual-provider-model");
            let forwarded = generations[0].to_string();
            assert!(!forwarded.contains(client["client_token"].as_str().unwrap()));
            assert!(!forwarded.contains(ready["request_token"].as_str().unwrap()));
        }
        assert!(process.stop().await);
    }
}

#[tokio::test]
async fn companion_missing_upstream_credential_never_sends_an_unauthenticated_generation() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let different_directory = tempfile::tempdir().unwrap();
    let id = store(
        different_directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), false);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (status, response) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(status, 502);
    assert_eq!(response, json!({"error":"provider_auth_unavailable"}));
    assert_private(&response, &id, &fixture.provider_url);
    fixture.wait_status(|state| state["running"] == 0).await;
    assert!(provider.generations.lock().unwrap().is_empty());
    assert!(provider.metadata.lock().unwrap().is_empty());
    assert!(process.stop().await);
}

#[tokio::test]
async fn presence_refresh_with_missing_credential_never_falls_back_to_anonymous_metadata() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let different_directory = tempfile::tempdir().unwrap();
    let id = store(
        different_directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), true);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (_, result) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_ne!(result["state"], "ready");
    assert_private(&result, &id, &fixture.provider_url);
    assert!(provider.metadata.lock().unwrap().is_empty());
    assert!(provider.generations.lock().unwrap().is_empty());
    assert!(process.stop().await);
}

#[tokio::test]
async fn companion_does_not_send_credentials_to_a_changed_endpoint_or_profile() {
    for endpoint_changed in [true, false] {
        let (mut fixture, provider) = fixture().await;
        let directory = tempfile::tempdir().unwrap();
        let credential_endpoint = if endpoint_changed {
            "http://127.0.0.1:1"
        } else {
            &fixture.provider_url
        };
        let credential_profile = if endpoint_changed {
            "openai_compatible_chat"
        } else {
            "lmstudio_chat"
        };
        let id = store(
            directory.path(),
            credential_endpoint,
            credential_profile,
            KEY,
        );
        configure(&fixture, "openai_compatible_chat", id.clone(), false);
        let process = fixture.companion(directory.path()).await;
        let client = fixture.register().await;
        let (status, response) = fixture.prepare(&client, "main", "turn", "request").await;
        assert_eq!(status, 502);
        assert_eq!(response, json!({"error":"provider_auth_unavailable"}));
        assert_private(&response, &id, &fixture.provider_url);
        fixture.wait_status(|state| state["running"] == 0).await;
        assert!(provider.generations.lock().unwrap().is_empty());
        assert!(provider.metadata.lock().unwrap().is_empty());
        assert!(process.stop().await);
    }
}

#[tokio::test]
async fn authenticated_upstream_401_is_sanitized_and_never_retried_without_key() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), false);
    provider.reject.store(true, Ordering::SeqCst);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (_, ready) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(ready["state"], "ready");
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body, json!({"error":"provider_rejected"}));
    assert_private(&body, &id, &fixture.provider_url);
    let state = fixture.wait_status(|state| state["running"] == 0).await;
    assert_eq!(state["quarantined_pools"], json!([]));
    {
        let generations = provider.generations.lock().unwrap();
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["authorization"], format!("Bearer {KEY}"));
    }
    assert!(process.stop().await);
}

#[tokio::test]
async fn presence_refresh_does_not_merge_deployments_with_different_credentials() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    let other_id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        OTHER_KEY,
    );
    configure(&fixture, "openai_compatible_chat", id, true);
    {
        let mut core = fixture.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        let mut other = catalog.deployments[0].clone();
        other.id = "other-credential-deployment".into();
        other.upstream_credential_id = Some(other_id);
        catalog.deployments.push(other);
        core.replace_catalog(catalog, now_ms()).unwrap();
    }
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("AUTHENTICATED_MODEL_OK")
    );
    fixture.wait_status(|state| state["running"] == 0).await;
    let mut observed = provider.metadata.lock().unwrap().clone();
    observed.sort();
    let mut expected = vec![
        Some(format!("Bearer {KEY}")),
        Some(format!("Bearer {OTHER_KEY}")),
    ];
    expected.sort();
    assert_eq!(observed, expected);
    assert_eq!(provider.generations.lock().unwrap().len(), 1);
    assert!(process.stop().await);
}

#[tokio::test]
async fn admitted_request_keeps_its_credential_when_the_saved_key_is_removed() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), false);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    upstream_auth::remove(directory.path(), &id).unwrap();
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("AUTHENTICATED_MODEL_OK")
    );
    fixture.wait_status(|state| state["running"] == 0).await;
    {
        let generations = provider.generations.lock().unwrap();
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["authorization"], format!("Bearer {KEY}"));
    }
    assert_eq!(
        fixture
            .post(
                "/v1/turns/finish",
                client["client_token"].as_str().unwrap(),
                json!({"id":client["id"],"context":"main","turn_id":"turn"})
            )
            .await
            .0,
        200
    );
    let (status, response) = fixture
        .prepare(&client, "side_chat", "next-turn", "next-request")
        .await;
    assert_eq!(status, 502);
    assert_eq!(response, json!({"error":"provider_auth_unavailable"}));
    assert_eq!(provider.generations.lock().unwrap().len(), 1);
    assert!(process.stop().await);
}

fn rotate_catalog_key(fixture: &Fixture, directory: &Path, previous_id: &str) {
    let next_id = store(
        directory,
        &fixture.provider_url,
        "openai_compatible_chat",
        OTHER_KEY,
    );
    {
        let mut core = fixture.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        catalog.deployments[0].upstream_credential_id = Some(next_id);
        core.replace_catalog(catalog, now_ms()).unwrap();
    }
    upstream_auth::remove(directory, previous_id).unwrap();
}

#[tokio::test]
async fn catalog_key_rotation_requires_review_before_claiming_a_previously_ready_request() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), false);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    rotate_catalog_key(&fixture, directory.path(), &id);
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"review_required"})
    );
    fixture.wait_status(|state| state["running"] == 0).await;
    assert!(provider.generations.lock().unwrap().is_empty());
    assert!(process.stop().await);
}

#[tokio::test]
async fn catalog_key_rotation_does_not_interrupt_an_already_claimed_generation() {
    let (mut fixture, provider) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let id = store(
        directory.path(),
        &fixture.provider_url,
        "openai_compatible_chat",
        KEY,
    );
    configure(&fixture, "openai_compatible_chat", id.clone(), false);
    provider.hold_response.store(true, Ordering::SeqCst);
    let process = fixture.companion(directory.path()).await;
    let client = fixture.register().await;
    let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    let response = fixture.generate(&ready).await;
    assert_eq!(response.status(), StatusCode::OK);
    fixture.wait_status(|state| state["running"] == 1).await;
    rotate_catalog_key(&fixture, directory.path(), &id);
    provider.release_response.notify_one();
    let completed = tokio::time::timeout(Duration::from_secs(3), response.text())
        .await
        .unwrap()
        .unwrap();
    assert!(completed.contains("AUTHENTICATED_MODEL_OK"));
    assert!(completed.contains("[DONE]"));
    fixture.wait_status(|state| state["running"] == 0).await;
    {
        let generations = provider.generations.lock().unwrap();
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0]["authorization"], format!("Bearer {KEY}"));
    }
    assert!(process.stop().await);
}
