use super::*;

pub(super) async fn generation(
    State(provider): State<Provider>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    provider
        .records
        .lock()
        .unwrap()
        .push(json!({"body":body,"authorization_present":headers.contains_key("authorization")}));
    let status = body["instructions"].as_str().unwrap().to_owned();
    let (send, receive) = mpsc::channel(4);
    tokio::spawn(async move {
        let _ = send.send(Ok(Bytes::from_static(b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"response-test\",\"status\":\"in_progress\"}}\n\n"))).await;
        if provider.held {
            provider.release.notified().await;
        }
        if !provider.truncated {
            let event = json!({"type":format!("response.{status}"),"response":{"id":"response-test","status":status,
                "output":[{"type":"function_call","id":"call-item","call_id":"call-1","name":"inspect","arguments":"{\"path\":\"file\"}"}]}});
            let terminal = format!("event: response.{status}\r\ndata: {event}\r\n\r\n");
            // Force boundaries within the event and CRLF pair. Gateway forwards all bytes.
            for part in terminal.as_bytes().chunks(7) {
                if send.send(Ok(Bytes::copy_from_slice(part))).await.is_err() {
                    break;
                }
            }
        }
    });
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(Stream(receive)),
    )
        .into_response()
}

fn profile(fixture: &Fixture, profile: &str) {
    let mut core = fixture.core.lock().unwrap();
    let mut catalog = core.catalog_store().catalog.clone();
    catalog.deployments[0].provider_profile = profile.into();
    core.replace_catalog(catalog, now_ms()).unwrap();
    core.observe_present_deployment("deployment-qwen", Availability::Unknown, now_ms())
        .unwrap();
}
fn body(ready: &Value, status: &str) -> Value {
    json!({"model":ready["logical_model_id"],"instructions":status,
        "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"PROMPT_CANARY"}]},
            {"type":"function_call","call_id":"prior-call","name":"inspect","arguments":"{}"},
            {"type":"function_call_output","call_id":"prior-call","output":"RESULT_CANARY"}],
        "tools":[{"type":"function","name":"inspect","parameters":{"type":"object"}}],
        "store":false,"stream":true})
}
async fn request(
    fixture: &Fixture,
    ready: &Value,
    suffix: &str,
    token: &str,
    body: &Value,
) -> reqwest::Response {
    fixture
        .client
        .post(format!(
            "{}/{suffix}",
            ready["gateway_base_url"].as_str().unwrap()
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn responses_profiles_forward_full_input_without_translation_or_native_load_calls() {
    for mode in ["openai_compatible_responses", "lmstudio_responses"] {
        let fixture = Fixture::new(false, false).await;
        profile(&fixture, mode);
        let client = fixture.register().await;
        let (status, ready) = fixture.prepare(&client, "main", "turn", "request").await;
        assert_eq!(status, 200);
        assert_eq!(ready["provider_profile"], "openai_compatible_responses");
        assert_eq!(ready["provider_api_mode"], "responses");
        let original = body(&ready, "completed");
        let response = request(
            &fixture,
            &ready,
            "responses",
            ready["request_token"].as_str().unwrap(),
            &original,
        )
        .await;
        assert_eq!(response.status(), 200);
        let stream = response.text().await.unwrap();
        assert!(stream.contains("response.completed"));
        assert!(stream.contains("function_call"));
        assert!(!stream.contains("[DONE]"));
        fixture.wait_status(|s| s["running"] == 0).await;
        let records = fixture.provider.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        let mut expected = original;
        expected["model"] = json!("actual-provider-model");
        assert_eq!(records[0]["body"], expected);
        assert_eq!(records[0]["authorization_present"], false);
        assert_eq!(
            fixture.provider.unexpected_requests.load(Ordering::SeqCst),
            0
        );
    }
}

#[tokio::test]
async fn responses_wrong_token_api_and_stateful_body_do_not_consume_or_forward_a_permit() {
    let fixture = Fixture::new(false, false).await;
    profile(&fixture, "openai_compatible_responses");
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let original = body(&ready, "completed");
    let token = ready["request_token"].as_str().unwrap();
    assert_eq!(
        request(&fixture, &ready, "responses", "wrong-token", &original)
            .await
            .status(),
        401
    );
    assert_eq!(
        request(&fixture, &ready, "chat/completions", token, &original)
            .await
            .status(),
        400
    );
    for (field, value) in [
        ("store", json!(true)),
        ("previous_response_id", json!("old-response")),
        ("conversation", json!("old-conversation")),
        ("background", json!(true)),
        ("input", json!({})),
        ("stream", json!(false)),
        ("model", json!("other-model")),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        assert_eq!(
            request(&fixture, &ready, "responses", token, &invalid)
                .await
                .status(),
            400,
            "{field}"
        );
    }
    assert!(fixture.provider.records.lock().unwrap().is_empty());
    assert!(
        request(&fixture, &ready, "responses", token, &original)
            .await
            .text()
            .await
            .unwrap()
            .contains("response.completed")
    );
    fixture.wait_status(|s| s["running"] == 0).await;
    assert_eq!(
        request(&fixture, &ready, "responses", token, &original)
            .await
            .status(),
        409
    );
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);

    let chat = Fixture::new(false, false).await;
    let client = chat.register().await;
    let ready = chat
        .prepare(&client, "main", "chat-turn", "chat-request")
        .await
        .1;
    assert_eq!(
        request(
            &chat,
            &ready,
            "responses",
            ready["request_token"].as_str().unwrap(),
            &body(&ready, "completed")
        )
        .await
        .status(),
        400
    );
    assert!(chat.provider.records.lock().unwrap().is_empty());

    let mismatched = moyai_hub::gateway::Grant {
        permit: moyai_hub::protocol::ExecutionPermit {
            id: "mismatched-permit".into(),
            client_id: client["id"].as_str().unwrap().into(),
            context_id: "main".into(),
            lease_id: ready["lease_id"].as_str().unwrap().into(),
            request_id: "mismatched-request".into(),
            turn_id: "chat-turn".into(),
            catalog_revision: 1,
            logical_model_id: "logical-qwen".into(),
            deployment_id: "deployment-qwen".into(),
            provider_profile: "openai_compatible_responses".into(),
            expires_at_ms: now_ms() + 10_000,
            advisory_only: true,
        },
        deployment: chat
            .core
            .lock()
            .unwrap()
            .catalog_store()
            .catalog
            .deployments[0]
            .clone(),
        pool_limit: 1,
        global_limit: 1,
        device_certificate_sha256: None,
        device_admission_revision: None,
    };
    assert!(
        chat.control.grant(&mismatched).await.is_err(),
        "a profile not bound to this deployment cannot be admitted"
    );
}

#[tokio::test]
async fn responses_failure_incomplete_and_truncated_streams_settle_without_replay() {
    for status in ["failed", "incomplete", "truncated"] {
        let fixture = Fixture::new(false, status == "truncated").await;
        profile(&fixture, "openai_compatible_responses");
        let client = fixture.register().await;
        let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
        let response = request(
            &fixture,
            &ready,
            "responses",
            ready["request_token"].as_str().unwrap(),
            &body(&ready, status),
        )
        .await;
        let text = response.text().await;
        if status == "truncated" {
            assert!(text.is_err());
        } else {
            assert!(text.unwrap().contains(&format!("response.{status}")));
        }
        fixture
            .wait_status(|s| s["running"] == 0 && s["reserved"] == 0)
            .await;
        assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn cancelling_responses_closes_only_its_gateway_transport_without_replay() {
    let fixture = Fixture::new(true, false).await;
    profile(&fixture, "lmstudio_responses");
    let client = fixture.register().await;
    let ready = fixture.prepare(&client, "main", "turn", "request").await.1;
    let mut response = request(
        &fixture,
        &ready,
        "responses",
        ready["request_token"].as_str().unwrap(),
        &body(&ready, "completed"),
    )
    .await;
    assert!(response.chunk().await.unwrap().is_some());
    let closed = fixture
        .post(
            "/v1/turns/cancel",
            client["client_token"].as_str().unwrap(),
            json!({"id":client["id"],"context":"main","turn_id":"turn"}),
        )
        .await;
    assert_eq!(closed.0, 200);
    fixture
        .wait_status(|s| s["running"] == 0 && s["reserved"] == 0)
        .await;
    let _ = response.bytes().await;
    assert_eq!(fixture.provider.records.lock().unwrap().len(), 1);
    // No provider-completion wait: its fixture is still held until drop.
}
