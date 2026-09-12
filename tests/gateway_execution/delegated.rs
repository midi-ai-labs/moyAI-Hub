use super::*;

fn preparation(client: &Value, execution: &str) -> Value {
    json!({"id":client["id"],"context":"main","turn_id":execution,"request_id":"child-request",
        "expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"],"purpose":"delegated",
        "delegated_selection":{"allowed_model_ids":["logical-qwen"],"preferred_model_id":"logical-qwen",
            "required_capabilities":["tools"],"wait_policy":"wait_for_preferred","affinity_turns":2}})
}
async fn close(
    fixture: &Fixture,
    client: &Value,
    execution: &str,
    delegated: bool,
) -> (u16, Value) {
    fixture
        .post(
            "/v1/turns/cancel",
            client["client_token"].as_str().unwrap(),
            json!({"id":client["id"],"context":"main","turn_id":execution,"delegated":delegated}),
        )
        .await
}

#[tokio::test]
async fn delegated_requests_hold_independent_gateway_scopes_and_heartbeat_owners() {
    let fixture = Fixture::new(true, false).await;
    {
        let mut core = fixture.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        catalog.policy.global_max_concurrent = 3;
        catalog.capacity_pools[0].max_concurrent = 3;
        core.replace_catalog(catalog, now_ms()).unwrap();
    }
    let client = fixture.register().await;
    assert_eq!(client["supports_delegated_execution"], true);
    let token = client["client_token"].as_str().unwrap();
    let parent = fixture
        .prepare(&client, "main", "parent", "parent-request")
        .await
        .1;
    let parent_response = fixture.generate(&parent).await;
    let first = ulid::Ulid::new().to_string();
    let second = ulid::Ulid::new().to_string();
    let ready1 = fixture
        .post("/v1/requests/prepare", token, preparation(&client, &first))
        .await;
    assert_eq!(ready1.0, 200);
    assert_eq!(ready1.1["state"], "ready");
    let response1 = fixture.generate(&ready1.1).await;
    let ready2 = fixture
        .post("/v1/requests/prepare", token, preparation(&client, &second))
        .await;
    assert_eq!(ready2.0, 200);
    assert_eq!(ready2.1["state"], "ready");
    let response2 = fixture.generate(&ready2.1).await;
    fixture.wait_status(|s| s["running"] == 3).await;
    assert_eq!(fixture.post("/v1/clients/heartbeat",token,json!({"id":client["id"],"activity":"running",
        "active_turns":[{"context":"main","turn_id":"parent"},{"context":"main","turn_id":first,"delegated":true},{"context":"main","turn_id":second,"delegated":true}]})).await.0,200);
    assert_eq!(close(&fixture, &client, "parent", false).await.0, 200);
    fixture.wait_status(|s| s["running"] == 2).await;
    assert_eq!(close(&fixture, &client, &first, true).await.0, 200);
    fixture.wait_status(|s| s["running"] == 1).await;
    assert_eq!(
        fixture
            .post("/v1/requests/prepare", token, preparation(&client, &first))
            .await
            .1["error"],
        "turn_closed"
    );
    assert_eq!(close(&fixture, &client, &second, true).await.0, 200);
    fixture.wait_status(|s| s["running"] == 0).await;
    drop((parent_response, response1, response2));
    assert_eq!(
        fixture.provider.records.lock().unwrap().len(),
        3,
        "cancellation never replays generation"
    );
    assert_eq!(
        fixture.core.lock().unwrap().client_rows(now_ms())[0]
            .contexts
            .len(),
        2
    );
}

#[tokio::test]
async fn delegated_prepare_rejects_missing_capability_shape_rebinding_and_cancel_before_admission()
{
    let fixture = Fixture::new(false, false).await;
    let client = fixture.register().await;
    let token = client["client_token"].as_str().unwrap();
    let execution = ulid::Ulid::new().to_string();
    let valid = preparation(&client, &execution);
    let mut mismatched = valid.clone();
    mismatched["delegated_selection"]["affinity_turns"] = json!(1);
    assert_eq!(
        fixture
            .post("/v1/requests/prepare", token, mismatched)
            .await
            .1["error"],
        "review_required"
    );
    for field in ["purpose", "delegated_selection"] {
        let mut absent = valid.clone();
        absent.as_object_mut().unwrap().remove(field);
        assert_eq!(
            fixture.post("/v1/requests/prepare", token, absent).await.0,
            400
        );
    }
    let mut invalid_id = valid.clone();
    invalid_id["turn_id"] = json!("not-an-execution-ulid");
    assert_eq!(
        fixture
            .post("/v1/requests/prepare", token, invalid_id)
            .await
            .0,
        400
    );
    assert_eq!(
        fixture
            .post("/v1/requests/prepare", "other-client-token", valid.clone())
            .await
            .0,
        401
    );
    assert!(fixture.provider.records.lock().unwrap().is_empty());
    assert_eq!(close(&fixture, &client, &execution, true).await.0, 200);
    assert_eq!(
        fixture.post("/v1/requests/prepare", token, valid).await.1["error"],
        "turn_closed"
    );
    let second = ulid::Ulid::new().to_string();
    let ready = fixture
        .post("/v1/requests/prepare", token, preparation(&client, &second))
        .await;
    assert_eq!(ready.0, 200);
    assert_eq!(ready.1["state"], "ready");
    assert!(
        fixture
            .generate(&ready.1)
            .await
            .text()
            .await
            .unwrap()
            .contains("MODEL_OK")
    );
    fixture.wait_status(|s| s["running"] == 0).await;
    assert_eq!(close(&fixture, &client, &second, true).await.0, 200);
    assert_eq!(
        fixture
            .post("/v1/requests/prepare", token, preparation(&client, &second))
            .await
            .1["error"],
        "turn_closed"
    );
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
}
