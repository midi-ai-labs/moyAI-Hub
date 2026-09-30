use moyai_hub::{
    catalog::{CatalogStore, LogicalModel},
    core::HubCore,
    service::{SharedCore, router},
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

const BOOTSTRAP: &str = "bootstrap-secret-0123456789-0123456789";

struct Server {
    core: SharedCore,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new() -> Self {
        let mut store = CatalogStore::empty("0.1.0");
        let mut catalog = store.catalog.clone();
        catalog.models = vec![
            LogicalModel {
                system_prompt: String::new(),
                id: "model-a".into(),
                label: "Model A".into(),
                capabilities: BTreeSet::from(["tools".into()]),
            },
            LogicalModel {
                system_prompt: String::new(),
                id: "model-b".into(),
                label: "Model B".into(),
                capabilities: BTreeSet::from(["tools".into(), "vision".into()]),
            },
        ];
        store.replace_catalog(catalog, 0).unwrap();
        Self::from_core(Arc::new(Mutex::new(HubCore::new(store).unwrap()))).await
    }

    async fn from_core(core: SharedCore) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let serving_core = core.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(serving_core, BOOTSTRAP.into())
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self { core, base, task }
    }

    async fn register(&self, label: &str) -> Value {
        Client::new()
            .post(format!("{}/v1/clients/register", self.base))
            .bearer_auth(BOOTSTRAP)
            .json(&json!({"label":label}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn post(&self, path: &str, token: &str, body: &Value) -> (StatusCode, Value) {
        let response = Client::new()
            .post(format!("{}{path}", self.base))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }
}

fn review_body(client: &Value, context: &str) -> Value {
    json!({"id":client["id"],"context":context,"expected_hub_id":client["hub_id"],"reviewed_revision":client["revision"],
        "selection":{"allowed_model_ids":["model-a"],"preferred_model_id":"model-a","required_capabilities":["tools"],"wait_policy":"wait_for_preferred","affinity_turns":2}})
}

#[tokio::test]
async fn authenticated_heartbeat_renews_only_its_exact_live_turn_and_accepts_legacy_clients() {
    use moyai_hub::{
        catalog::{CapacityPool, Deployment},
        protocol::{Availability, RouteDecision},
    };
    let server = Server::new().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    {
        let mut core = server.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        catalog.deployments.push(Deployment {
            id: "host".into(),
            logical_model_id: "model-a".into(),
            capacity_pool_id: "pool".into(),
            label: "Host".into(),
            endpoint: "http://127.0.0.1:1234/v1".into(),
            provider_model: "model-a".into(),
            provider_profile: "openai_compatible_chat".into(),
            upstream_credential_id: None,
            enabled: true,
        });
        catalog.capacity_pools.push(CapacityPool {
            id: "pool".into(),
            label: "Pool".into(),
            max_concurrent: 1,
        });
        catalog.policy.lease_ttl_ms = 5000;
        core.replace_catalog(catalog, now).unwrap();
        core.observe_deployment("host", Availability::Ready, now)
            .unwrap();
    }
    let a = server.register("A").await;
    let b = server.register("B").await;
    let token = a["client_token"].as_str().unwrap();
    let id = a["id"].as_str().unwrap();
    assert_eq!(
        server
            .post("/v1/clients/review", token, &review_body(&a, "main"))
            .await
            .0,
        StatusCode::OK
    );
    let RouteDecision::Assigned { lease: original } = server
        .core
        .lock()
        .unwrap()
        .begin_turn(id, "main", "turn", now - 4000)
        .unwrap()
    else {
        panic!("expected route")
    };
    assert_eq!(server.post("/v1/clients/heartbeat", token, &json!({"id":b["id"],"activity":"running","active_turns":[{"context":"main","turn_id":"turn"}]})).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server
            .post(
                "/v1/clients/heartbeat",
                token,
                &json!({"id":id,"activity":"waiting"})
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        server
            .core
            .lock()
            .unwrap()
            .begin_turn(id, "main", "turn", now)
            .unwrap(),
        RouteDecision::Assigned {
            lease: original.clone()
        }
    );
    assert_eq!(server.post("/v1/clients/heartbeat", token, &json!({"id":id,"activity":"waiting","active_turns":[{"context":"main","turn_id":"turn"}]})).await.0, StatusCode::OK);
    let RouteDecision::Assigned { lease: renewed } = server
        .core
        .lock()
        .unwrap()
        .begin_turn(id, "main", "turn", now + 1001)
        .unwrap()
    else {
        panic!("expected renewed route")
    };
    assert_eq!(renewed.id, original.id);
    assert!(renewed.expires_at_ms > original.expires_at_ms);
    server
        .core
        .lock()
        .unwrap()
        .finish_turn(id, "main", "turn")
        .unwrap();
    assert_eq!(server.post("/v1/clients/heartbeat", token, &json!({"id":id,"activity":"waiting","active_turns":[{"context":"main","turn_id":"turn"}]})).await.0, StatusCode::OK);
    assert_eq!(
        server
            .core
            .lock()
            .unwrap()
            .begin_turn(id, "main", "turn", now + 1001),
        Err(moyai_hub::protocol::HubError::TurnClosed)
    );
}

#[tokio::test]
async fn registration_binds_hub_and_client_token_reads_catalog_but_only_own_reviews() {
    let server = Server::new().await;
    let a = server.register("Desktop A").await;
    let b = server.register("Desktop B").await;
    assert_eq!(
        a["hub_id"],
        server.core.lock().unwrap().catalog_store().hub_id
    );
    assert_eq!(a["revision"], "2");
    assert_eq!(a["supports_turn_heartbeat"], true);
    let token = a["client_token"].as_str().unwrap();
    let catalog: Value = Client::new()
        .get(format!("{}/v1/catalog", server.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["hub_id"], a["hub_id"]);
    for private in [
        "deployments",
        "endpoint",
        "client_token",
        "prompt",
        "response",
    ] {
        assert!(catalog.get(private).is_none());
    }
    assert_eq!(
        server
            .post("/v1/clients/review", BOOTSTRAP, &review_body(&a, "main"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post("/v1/clients/review", token, &review_body(&b, "main"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, ack) = server
        .post("/v1/clients/review", token, &review_body(&a, "main"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ack,
        json!({"id":a["id"],"context":"main","reviewed_revision":"2"})
    );
    assert_eq!(
        server
            .post("/v1/clients/review", token, &review_body(&a, "main"))
            .await
            .1,
        ack
    );
    assert_eq!(
        server
            .post("/v1/clients/review", token, &review_body(&a, "side_chat"))
            .await
            .0,
        StatusCode::OK
    );
    let rows = server.core.lock().unwrap().client_rows(0);
    assert_eq!(
        rows.iter()
            .find(|row| row.id == a["id"])
            .unwrap()
            .contexts
            .len(),
        2
    );
    assert!(
        rows.iter()
            .find(|row| row.id == b["id"])
            .unwrap()
            .contexts
            .is_empty()
    );
}

#[tokio::test]
async fn changed_revision_requires_each_context_to_review_and_rejects_different_hub() {
    let server = Server::new().await;
    let a = server.register("Desktop").await;
    let token = a["client_token"].as_str().unwrap();
    for context in ["main", "side_chat"] {
        assert_eq!(
            server
                .post("/v1/clients/review", token, &review_body(&a, context))
                .await
                .0,
            StatusCode::OK
        );
    }
    {
        let mut core = server.core.lock().unwrap();
        let mut catalog = core.catalog_store().catalog.clone();
        catalog.models[0].label = "Updated".into();
        core.replace_catalog(catalog, 1).unwrap();
    }
    assert_eq!(
        server
            .post("/v1/clients/review", token, &review_body(&a, "main"))
            .await,
        (
            StatusCode::CONFLICT,
            json!({"error":"review_required","current_revision":"3"})
        )
    );
    let mut changed = review_body(&a, "main");
    changed["reviewed_revision"] = json!("3");
    changed["expected_hub_id"] = json!("different-hub");
    assert_eq!(
        server.post("/v1/clients/review", token, &changed).await,
        (
            StatusCode::CONFLICT,
            json!({"error":"different_hub","current_revision":"3"})
        )
    );
    changed["expected_hub_id"] = a["hub_id"].clone();
    assert_eq!(
        server.post("/v1/clients/review", token, &changed).await.0,
        StatusCode::OK
    );
    let rows = server.core.lock().unwrap().client_rows(1);
    let contexts = &rows[0].contexts;
    assert!(
        !contexts
            .iter()
            .find(|context| context.context_id == "main")
            .unwrap()
            .review_required
    );
    assert!(
        contexts
            .iter()
            .find(|context| context.context_id == "side_chat")
            .unwrap()
            .review_required
    );
}

#[tokio::test]
async fn strict_review_dto_rejects_noncanonical_or_unsafe_settings_without_saving() {
    let server = Server::new().await;
    let a = server.register("Desktop").await;
    let token = a["client_token"].as_str().unwrap();
    for invalid_revision in [
        json!(2),
        json!("02"),
        json!("0"),
        json!("+2"),
        json!("18446744073709551616"),
    ] {
        let mut body = review_body(&a, "main");
        body["reviewed_revision"] = invalid_revision;
        assert_eq!(
            server.post("/v1/clients/review", token, &body).await,
            (
                StatusCode::BAD_REQUEST,
                json!({"error":"invalid_selection"})
            )
        );
    }
    for (pointer, value) in [
        ("/context", json!("main-session")),
        ("/selection/wait_policy", json!("any_allowed")),
        ("/selection/allowed_model_ids", json!([])),
        (
            "/selection/allowed_model_ids",
            json!(["model-a", "model-a"]),
        ),
        (
            "/selection/required_capabilities",
            json!(["tools", "tools"]),
        ),
        ("/selection/affinity_turns", json!(101)),
        ("/selection/affinity_turns", json!(1.5)),
    ] {
        let mut body = review_body(&a, "main");
        *body.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            server.post("/v1/clients/review", token, &body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut body = review_body(&a, "main");
    body["selection"]["endpoint"] = json!("http://unexpected.example");
    assert_eq!(
        server.post("/v1/clients/review", token, &body).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut body = review_body(&a, "main");
    body["selection"]["allowed_model_ids"] = json!(["missing"]);
    body["selection"]["preferred_model_id"] = json!("missing");
    assert_eq!(
        server.post("/v1/clients/review", token, &body).await.1["error"],
        "model_removed"
    );
    let mut body = review_body(&a, "main");
    body["selection"]["required_capabilities"] = json!(["vision"]);
    assert_eq!(
        server.post("/v1/clients/review", token, &body).await.1["error"],
        "capability_mismatch"
    );
    assert!(
        server.core.lock().unwrap().client_rows(0)[0]
            .contexts
            .is_empty()
    );
    body["selection"]["allowed_model_ids"] = json!(["model-a", "model-b"]);
    body["selection"]["wait_policy"] = json!("allow_selected_fallback");
    assert_eq!(
        server.post("/v1/clients/review", token, &body).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn disconnect_revokes_only_owned_contexts_and_restart_does_not_restore_credentials() {
    let server = Server::new().await;
    let a = server.register("Desktop A").await;
    let b = server.register("Desktop B").await;
    let token = a["client_token"].as_str().unwrap();
    for context in ["main", "side_chat"] {
        server
            .post("/v1/clients/review", token, &review_body(&a, context))
            .await;
    }
    assert_eq!(
        server
            .post("/v1/clients/disconnect", token, &json!({"id":b["id"]}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post("/v1/clients/disconnect", BOOTSTRAP, &json!({"id":a["id"]}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post("/v1/clients/disconnect", token, &json!({"id":a["id"]}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        server
            .post("/v1/clients/disconnect", token, &json!({"id":a["id"]}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post(
                "/v1/clients/heartbeat",
                token,
                &json!({"id":a["id"],"activity":"running"})
            )
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        Client::new()
            .get(format!("{}/v1/catalog", server.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let rows = server.core.lock().unwrap().client_rows(0);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, b["id"]);
    let store = server.core.lock().unwrap().catalog_store().clone();
    let restarted = Server::from_core(Arc::new(Mutex::new(HubCore::new(store).unwrap()))).await;
    assert_eq!(
        restarted
            .post(
                "/v1/clients/review",
                b["client_token"].as_str().unwrap(),
                &review_body(&b, "main")
            )
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(restarted.core.lock().unwrap().client_rows(0).is_empty());
    let replacement = restarted.register("Desktop B").await;
    assert_ne!(replacement["id"], b["id"]);
    assert_eq!(replacement["hub_id"], b["hub_id"]);
}
