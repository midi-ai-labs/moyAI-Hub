use super::Admin;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

const PROFILE: &str = "openai_compatible_chat";
const KEY_A: &str = "admin-auth-test-first-key";
const KEY_B: &str = "admin-auth-test-replacement-key";
const STALE_KEY: &str = "admin-auth-test-stale-key-must-not-persist";

#[derive(Default)]
struct Pause {
    entered: Notify,
    resume: Notify,
}
#[derive(Default)]
struct ProviderState {
    required_key: Mutex<Option<String>>,
    received: Mutex<Vec<Option<String>>>,
    pause: Mutex<Option<Arc<Pause>>>,
}
struct Provider {
    endpoint: String,
    state: Arc<ProviderState>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Provider {
    async fn start(required_key: Option<&str>) -> Self {
        let state = Arc::new(ProviderState::default());
        *state.required_key.lock().unwrap() = required_key.map(str::to_owned);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/models", get(models))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            state,
            task,
        }
    }
    fn require(&self, key: Option<&str>) {
        *self.state.required_key.lock().unwrap() = key.map(str::to_owned);
    }
    fn last_authorization(&self) -> Option<String> {
        self.state.received.lock().unwrap().last().unwrap().clone()
    }
}
async fn models(
    State(state): State<Arc<ProviderState>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    state.received.lock().unwrap().push(authorization.clone());
    let expected = state
        .required_key
        .lock()
        .unwrap()
        .as_ref()
        .map(|key| format!("Bearer {key}"));
    let accepted = authorization == expected;
    let pause = state.pause.lock().unwrap().take();
    if let Some(pause) = pause {
        pause.entered.notify_one();
        pause.resume.notified().await;
    }
    if accepted {
        (
            StatusCode::OK,
            Json(json!({"data":[{"id":"model-a"},{"id":"model-b"}]})),
        )
    } else {
        // A provider may echo credentials in its errors; Hub must not forward this body.
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":format!("rejected {authorization:?}")})),
        )
    }
}
async fn snapshot(admin: &Arc<Admin>) -> Value {
    admin.dispatch("hub_snapshot", json!({})).await.unwrap()
}
async fn discover(
    admin: &Arc<Admin>,
    provider: &Provider,
    key: Option<&str>,
) -> Result<Value, String> {
    let mut args = json!({"endpoint":provider.endpoint,"providerProfile":PROFILE});
    if let Some(key) = key {
        args["apiKey"] = json!(key);
    }
    admin.dispatch("hub_discover", args).await
}
async fn register(
    admin: &Arc<Admin>,
    provider: &Provider,
    evidence: &Value,
    revision: &Value,
    model: &str,
) -> Result<Value, String> {
    admin
        .dispatch(
            "hub_register",
            json!({"request":{
                "revision":revision,"evidence_id":evidence["id"],"endpoint":provider.endpoint,
                "provider_profile":PROFILE,"model_id":model,"label":"試験モデル",
                "logical_model_id":null,"capacity_pool_id":null,"capacity":1
            }}),
        )
        .await
}
async fn edit(
    admin: &Arc<Admin>,
    snapshot: &Value,
    key: Value,
    remove: bool,
    label: &str,
) -> Result<Value, String> {
    admin.dispatch("hub_edit", json!({"request":{
        "revision":snapshot["store"]["revision"],"deployment_id":snapshot["deployments"][0]["deployment"]["id"],
        "label":label,"capacity":1,"enabled":true,"remove":remove,"api_key":key
    }})).await
}
fn assert_no_keys(value: &Value) {
    let text = value.to_string();
    for key in [KEY_A, KEY_B, STALE_KEY] {
        assert!(!text.contains(key));
    }
}
fn disk_contains(directory: &Path, needle: &str) -> bool {
    // Inspect only this feature's durable content. The running Hub owns its
    // lock file and SQLite connection; neither belongs to provider credentials.
    let mut paths = ["catalog.json", "manifest.json"]
        .into_iter()
        .map(|name| directory.join(name))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    let credentials = directory.join("upstream-credentials");
    if credentials.exists() {
        paths.extend(
            std::fs::read_dir(credentials)
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        );
    }
    paths.into_iter().any(|path| {
        std::fs::read(path)
            .unwrap()
            .windows(needle.len())
            .any(|part| part == needle.as_bytes())
    })
}

#[tokio::test]
async fn model_system_prompt_registration_edit_reload_and_legacy_omission_share_logical_owner() {
    let provider = Provider::start(None).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let admin = Admin::open(path.clone()).unwrap();
    let initial = snapshot(&admin).await;
    let evidence = discover(&admin, &provider, None).await.unwrap();
    let prompt = "日本語で回答してください。\n  固有名詞は原文を使います。";
    let mut request = json!({"revision":initial["store"]["revision"],"evidence_id":evidence["id"],
        "endpoint":provider.endpoint,"provider_profile":PROFILE,"model_id":"model-a","label":"モデル",
        "logical_model_id":null,"capacity_pool_id":null,"capacity":1,"system_prompt":format!("  {prompt}\n")});
    admin
        .dispatch("hub_register", json!({"request":request}))
        .await
        .unwrap();
    let saved = snapshot(&admin).await;
    assert_eq!(
        saved["store"]["catalog"]["models"][0]["system_prompt"],
        prompt
    );
    let logical_id = saved["store"]["catalog"]["models"][0]["id"].clone();
    request["revision"] = saved["store"]["revision"].clone();
    request["logical_model_id"] = logical_id.clone();
    request["model_id"] = json!("model-b");
    request["system_prompt"] = Value::Null;
    admin
        .dispatch("hub_register", json!({"request":request}))
        .await
        .unwrap();
    let shared = snapshot(&admin).await;
    assert_eq!(
        shared["store"]["catalog"]["models"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(shared["deployments"].as_array().unwrap().len(), 2);
    let mut edit_request = json!({"revision":shared["store"]["revision"],
        "deployment_id":shared["deployments"][0]["deployment"]["id"],"label":"モデル",
        "capacity":1,"enabled":true,"remove":false,"system_prompt":"界".repeat(16_385)});
    assert!(
        admin
            .dispatch("hub_edit", json!({"request":edit_request}))
            .await
            .is_err()
    );
    assert_eq!(snapshot(&admin).await["store"], shared["store"]);
    edit_request["system_prompt"] = json!("日本語を基本とします。\n簡潔に回答してください。");
    admin
        .dispatch("hub_edit", json!({"request":edit_request}))
        .await
        .unwrap();
    let updated = snapshot(&admin).await;
    assert_eq!(
        updated["store"]["changes"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["changed_model_ids"],
        json!([logical_id])
    );
    admin.shutdown().await.unwrap();
    drop(admin);
    let admin = Admin::open(path).unwrap();
    let reopened = snapshot(&admin).await;
    assert_eq!(reopened["store"]["catalog"], updated["store"]["catalog"]);
    // Old management callers omitting the new field must preserve it.
    edit(&admin, &reopened, Value::Null, false, "名前だけ変更")
        .await
        .unwrap();
    let renamed = snapshot(&admin).await;
    assert_eq!(
        renamed["store"]["catalog"]["models"][0]["system_prompt"],
        edit_request["system_prompt"]
    );
    edit_request["revision"] = renamed["store"]["revision"].clone();
    edit_request["system_prompt"] = json!(" \r\n\t ");
    admin
        .dispatch("hub_edit", json!({"request":edit_request}))
        .await
        .unwrap();
    assert!(
        snapshot(&admin).await["store"]["catalog"]["models"][0]
            .get("system_prompt")
            .is_none()
    );
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn authenticated_model_lifecycle_preserves_updates_clears_and_removes_keys_without_projection()
 {
    let provider = Provider::start(Some(KEY_A)).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let admin = Admin::open(path.clone()).unwrap();
    let initial = snapshot(&admin).await;
    assert!(
        discover(&admin, &provider, None)
            .await
            .unwrap_err()
            .contains("401")
    );
    let rejected = discover(&admin, &provider, Some(KEY_B)).await.unwrap_err();
    assert!(rejected.contains("401"));
    assert!(!rejected.contains(KEY_B));
    let evidence = discover(&admin, &provider, Some(KEY_A)).await.unwrap();
    assert_no_keys(&evidence);
    assert!(evidence.get("api_key").is_none());
    register(
        &admin,
        &provider,
        &evidence,
        &initial["store"]["revision"],
        "model-a",
    )
    .await
    .unwrap();
    let registered = snapshot(&admin).await;
    assert_no_keys(&registered);
    assert_eq!(registered["deployments"][0]["model_present"], true);
    assert!(!std::fs::read_to_string(&path).unwrap().contains(KEY_A));
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_A}"))
    );

    // A label change with null/omitted key is not a request to clear saved authentication.
    edit(&admin, &registered, Value::Null, false, "名前を変更")
        .await
        .unwrap();
    admin.shutdown().await.unwrap();
    drop(admin);
    let admin = Admin::open(path.clone()).unwrap();
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_A}"))
    );
    let reopened = snapshot(&admin).await;
    assert_eq!(reopened["deployments"][0]["model_present"], true);
    assert_no_keys(&reopened);

    provider.require(Some(KEY_B));
    edit(&admin, &reopened, json!(KEY_B), false, "名前を変更")
        .await
        .unwrap();
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    let replaced = snapshot(&admin).await;
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_B}"))
    );
    assert_eq!(replaced["deployments"][0]["model_present"], true);
    assert!(!disk_contains(directory.path(), KEY_A));
    assert_no_keys(&replaced);

    let error = edit(
        &admin,
        &reopened,
        json!(STALE_KEY),
        false,
        "古い画面から変更",
    )
    .await
    .unwrap_err();
    assert!(!error.contains(STALE_KEY));
    assert_eq!(snapshot(&admin).await, replaced);
    assert!(!disk_contains(directory.path(), STALE_KEY));

    provider.require(None);
    edit(&admin, &replaced, json!(""), false, "認証なし")
        .await
        .unwrap();
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    let cleared = snapshot(&admin).await;
    assert_eq!(provider.last_authorization(), None);
    assert_eq!(cleared["deployments"][0]["model_present"], true);
    assert!(!disk_contains(directory.path(), KEY_B));

    provider.require(Some(KEY_B));
    edit(&admin, &cleared, json!(KEY_B), false, "削除試験")
        .await
        .unwrap();
    let restored_key = snapshot(&admin).await;
    edit(&admin, &restored_key, Value::Null, true, "削除試験")
        .await
        .unwrap();
    let removed = snapshot(&admin).await;
    assert!(removed["deployments"].as_array().unwrap().is_empty());
    assert!(
        removed["store"]["catalog"]["models"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!disk_contains(directory.path(), KEY_B));
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn rejected_registration_and_invalid_key_edits_preserve_current_catalog_and_storage() {
    let provider = Provider::start(Some(KEY_A)).await;
    let directory = tempfile::tempdir().unwrap();
    let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
    let initial = snapshot(&admin).await;
    let evidence = discover(&admin, &provider, Some(KEY_A)).await.unwrap();
    register(
        &admin,
        &provider,
        &evidence,
        &initial["store"]["revision"],
        "model-a",
    )
    .await
    .unwrap();
    let current = snapshot(&admin).await;
    assert!(
        register(
            &admin,
            &provider,
            &evidence,
            &initial["store"]["revision"],
            "model-b"
        )
        .await
        .is_err()
    );
    assert_eq!(snapshot(&admin).await, current);
    let invalid = format!("{STALE_KEY}\nvalue");
    let error = edit(&admin, &current, json!(invalid), false, "保存されない変更")
        .await
        .unwrap_err();
    assert!(!error.contains(STALE_KEY));
    assert_eq!(snapshot(&admin).await, current);
    assert!(!disk_contains(directory.path(), STALE_KEY));
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_A}"))
    );
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn refresh_from_previous_credential_does_not_overwrite_updated_model_state() {
    let provider = Provider::start(Some(KEY_A)).await;
    let directory = tempfile::tempdir().unwrap();
    let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
    let initial = snapshot(&admin).await;
    let evidence = discover(&admin, &provider, Some(KEY_A)).await.unwrap();
    register(
        &admin,
        &provider,
        &evidence,
        &initial["store"]["revision"],
        "model-a",
    )
    .await
    .unwrap();
    let registered = snapshot(&admin).await;
    let pause = Arc::new(Pause::default());
    *provider.state.pause.lock().unwrap() = Some(pause.clone());
    let refreshing = {
        let admin = admin.clone();
        tokio::spawn(async move { admin.dispatch("hub_refresh", json!({})).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), pause.entered.notified())
        .await
        .unwrap();
    provider.require(Some(KEY_B));
    edit(&admin, &registered, json!(KEY_B), false, "更新済み")
        .await
        .unwrap();
    let changed = snapshot(&admin).await;
    assert_eq!(changed["deployments"][0]["model_present"], false);
    pause.resume.notify_one();
    refreshing.await.unwrap().unwrap();
    assert_eq!(snapshot(&admin).await, changed);
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        snapshot(&admin).await["deployments"][0]["model_present"],
        true
    );
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_B}"))
    );
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn catalogs_without_credential_fields_reopen_and_refresh_without_authentication() {
    let provider = Provider::start(None).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let admin = Admin::open(path.clone()).unwrap();
    let initial = snapshot(&admin).await;
    let evidence = discover(&admin, &provider, None).await.unwrap();
    register(
        &admin,
        &provider,
        &evidence,
        &initial["store"]["revision"],
        "model-a",
    )
    .await
    .unwrap();
    admin.shutdown().await.unwrap();
    drop(admin);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        saved["catalog"]["deployments"][0]
            .get("upstream_credential_id")
            .is_none()
    );
    let reopened = Admin::open(path).unwrap();
    reopened.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        snapshot(&reopened).await["deployments"][0]["model_present"],
        true
    );
    assert_eq!(provider.last_authorization(), None);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn backups_exclude_upstream_keys_and_restore_requires_reentry_without_unauthenticated_fallback()
 {
    let provider = Provider::start(Some(KEY_A)).await;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let admin = Admin::open(source.join("catalog.json")).unwrap();
    let initial = snapshot(&admin).await;
    let evidence = discover(&admin, &provider, Some(KEY_A)).await.unwrap();
    register(
        &admin,
        &provider,
        &evidence,
        &initial["store"]["revision"],
        "model-a",
    )
    .await
    .unwrap();
    let registered = snapshot(&admin).await;
    admin.shutdown().await.unwrap();
    drop(admin);

    let backup = directory.path().join("backup");
    let restored = directory.path().join("restored");
    crate::backup::backup(&source, &backup).unwrap();
    assert!(!disk_contains(&backup, KEY_A));
    crate::backup::restore(&backup, &restored, true).unwrap();
    assert!(!disk_contains(&restored, KEY_A));
    let admin = Admin::open(restored.join("catalog.json")).unwrap();
    let before = provider.state.received.lock().unwrap().len();
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    let missing = snapshot(&admin).await;
    assert_eq!(provider.state.received.lock().unwrap().len(), before);
    assert_eq!(missing["deployments"][0]["model_present"], false);
    assert_eq!(
        missing["deployments"][0]["deployment"]["upstream_credential_id"],
        registered["deployments"][0]["deployment"]["upstream_credential_id"]
    );
    assert_no_keys(&missing);

    edit(&admin, &missing, json!(KEY_A), false, "APIキーを再設定")
        .await
        .unwrap();
    admin.dispatch("hub_refresh", json!({})).await.unwrap();
    assert_eq!(
        snapshot(&admin).await["deployments"][0]["model_present"],
        true
    );
    assert_eq!(
        provider.last_authorization(),
        Some(format!("Bearer {KEY_A}"))
    );
    admin.shutdown().await.unwrap();
}
