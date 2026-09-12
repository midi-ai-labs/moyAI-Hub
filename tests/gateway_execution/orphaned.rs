use super::*;

fn prepare_body(client: &Value, execution: &str, request: &str) -> Value {
    json!({"id":client["id"],"context":"main","turn_id":execution,"request_id":request,
        "expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"],"purpose":"delegated",
        "delegated_selection":{"allowed_model_ids":["logical-qwen"],"preferred_model_id":"logical-qwen",
            "required_capabilities":["tools"],"wait_policy":"wait_for_preferred","affinity_turns":2}})
}
fn short_presence(fixture: &Fixture) {
    let mut core = fixture.core.lock().unwrap();
    let mut catalog = core.catalog_store().catalog.clone();
    catalog.policy.heartbeat_stale_ms = 1000;
    catalog.policy.lease_ttl_ms = 2000;
    catalog.deployments[0].provider_profile = "lmstudio_chat".into();
    core.replace_catalog(catalog, now_ms()).unwrap();
}
fn heartbeat(fixture: &mut Fixture, client: &Value, live: Option<String>) -> Arc<AtomicUsize> {
    let http = fixture.client.clone();
    let hub = fixture.hub.clone();
    let client = client.clone();
    let failures = Arc::new(AtomicUsize::new(0));
    let counted = failures.clone();
    fixture.tasks.push(tokio::spawn(async move {
        loop {
            let active = live
                .as_ref()
                .map(|id| vec![json!({"context":"main","turn_id":id,"delegated":true})])
                .unwrap_or_default();
            let response = http
                .post(format!("{hub}/v1/clients/heartbeat"))
                .bearer_auth(client["client_token"].as_str().unwrap())
                .json(&json!({"id":client["id"],"activity":"running","active_turns":active}))
                .send()
                .await;
            if response.is_err() || !response.unwrap().status().is_success() {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }));
    failures
}

#[tokio::test]
async fn orphaned_close_failures_release_only_absent_children_while_client_heartbeat_continues() {
    let mut fixture = Fixture::new(false, false).await;
    short_presence(&fixture);
    let client = fixture.register().await;
    let token = client["client_token"].as_str().unwrap();
    let live = ulid::Ulid::new().to_string();
    let failures = heartbeat(&mut fixture, &client, Some(live.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let close_url = format!("http://{}/v1/turns/finish", listener.local_addr().unwrap());
    // A failing HTTP hop affects only close. No close reaches the real Hub; its
    // registration, generation and regular/one-live-child heartbeats still work.
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/turns/finish",
                post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            ),
        )
        .await
        .unwrap();
    }));
    let mut orphans = Vec::new();
    for index in 0..14 {
        let execution = if index == 0 {
            live.clone()
        } else {
            ulid::Ulid::new().to_string()
        };
        let (status, ready) = fixture
            .post(
                "/v1/requests/prepare",
                token,
                prepare_body(&client, &execution, "request"),
            )
            .await;
        assert_eq!(status, 200, "{ready}");
        assert_eq!(ready["state"], "ready");
        assert!(
            fixture
                .generate(&ready)
                .await
                .text()
                .await
                .unwrap()
                .contains("MODEL_OK")
        );
        fixture.wait_status(|s| s["running"] == 0).await;
        if index > 0 {
            assert_eq!(fixture.client.post(&close_url).bearer_auth(token).json(&json!({"id":client["id"],"context":"main","turn_id":execution,"delegated":true})).send().await.unwrap().status(),503);
            orphans.push(execution);
        }
    }
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert_eq!(failures.load(Ordering::SeqCst), 0);
    let new_id = ulid::Ulid::new().to_string();
    let (status, ready) = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &new_id, "request"),
        )
        .await;
    assert_eq!(
        status, 200,
        "absent child contexts must release the 14-slot limit: {ready}"
    );
    assert_eq!(ready["state"], "ready");
    assert!(
        fixture
            .generate(&ready)
            .await
            .text()
            .await
            .unwrap()
            .contains("MODEL_OK")
    );
    fixture.wait_status(|s| s["running"] == 0).await;
    for execution in &orphans {
        let rejected = fixture
            .post(
                "/v1/requests/prepare",
                token,
                prepare_body(&client, execution, "late-request"),
            )
            .await;
        assert_eq!(rejected.1["error"], "turn_closed");
    }
    let healthy = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &live, "live-next"),
        )
        .await;
    assert_eq!(healthy.0, 200);
    assert_eq!(
        healthy.1["state"], "ready",
        "explicit live-child heartbeat preserved its scope"
    );
    assert_eq!(
        fixture.provider.records.lock().unwrap().len(),
        15,
        "no generation replay while retiring metadata"
    );
}

#[tokio::test]
async fn orphaned_metadata_retirement_does_not_release_a_live_gateway_relay_slot() {
    let mut fixture = Fixture::new(true, false).await;
    short_presence(&fixture);
    let client = fixture.register().await;
    let token = client["client_token"].as_str().unwrap();
    let failures = heartbeat(&mut fixture, &client, None);
    let orphan = ulid::Ulid::new().to_string();
    let (_, ready) = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &orphan, "request"),
        )
        .await;
    let mut response = fixture.generate(&ready).await;
    assert!(response.chunk().await.unwrap().is_some());
    fixture.wait_status(|state| state["running"] == 1).await;
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert_eq!(failures.load(Ordering::SeqCst), 0);
    let next = ulid::Ulid::new().to_string();
    let waiting = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &next, "request"),
        )
        .await;
    assert_eq!(waiting.0, 200);
    assert_eq!(
        waiting.1["state"], "waiting",
        "Gateway still owns its held HTTP relay capacity"
    );
    assert_eq!(fixture.control.status().await.unwrap()["running"], 1);
    let replay = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &orphan, "late-request"),
        )
        .await;
    assert_eq!(replay.1["error"], "turn_closed");
    drop(response);
    fixture.wait_status(|state| state["running"] == 0).await;
    let ready = fixture
        .post(
            "/v1/requests/prepare",
            token,
            prepare_body(&client, &next, "request"),
        )
        .await;
    assert_eq!(ready.0, 200);
    assert_eq!(ready.1["state"], "ready");
    assert_eq!(
        fixture.provider.records.lock().unwrap().len(),
        1,
        "no provider replay or inference completion probe"
    );
}
