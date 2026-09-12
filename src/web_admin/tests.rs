use super::*;

#[test]
fn anonymous_csrf_is_origin_bound_and_expires_with_listener_epoch() {
    let access = WebAccess::default();
    assert!(!access.enabled());
    access.enable().unwrap();
    let epoch = access.epoch().unwrap();
    let local = WebPolicy::new("http://127.0.0.1:3000", epoch.clone()).unwrap();
    let remote = WebPolicy::new("http://192.168.10.1:3000", epoch).unwrap();
    let empty = HeaderMap::new();
    let csrf = access.session(&empty, false, &local).unwrap();
    assert_eq!(
        access.session(&empty, true, &local),
        Err(StatusCode::FORBIDDEN)
    );
    let mut headers = HeaderMap::new();
    headers.insert("x-moyai-csrf", csrf.parse().unwrap());
    assert_eq!(access.session(&headers, true, &local).unwrap(), csrf);
    assert_eq!(
        access.session(&headers, true, &remote),
        Err(StatusCode::FORBIDDEN)
    );
    assert_ne!(access.session(&empty, false, &remote).unwrap(), csrf);
    access.enable().unwrap();
    assert_eq!(
        access.session(&headers, true, &local),
        Err(StatusCode::NOT_FOUND)
    );
    let renewed = WebPolicy::new(&local.origin, access.epoch().unwrap()).unwrap();
    assert_eq!(
        access.session(&headers, true, &renewed),
        Err(StatusCode::FORBIDDEN)
    );
    access.disable();
    assert_eq!(
        access.session(&empty, false, &renewed),
        Err(StatusCode::NOT_FOUND)
    );
}

#[test]
fn browser_origin_checks_reject_cross_origin_and_host_rebinding() {
    let peer = WebPolicy::new("https://127.0.0.1:9471", "TestAccess".into()).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, peer.authority.parse().unwrap());
    assert!(origin_allowed(&headers, &peer, &Method::GET));
    assert!(!origin_allowed(&headers, &peer, &Method::POST));
    headers.insert(header::ORIGIN, "https://127.0.0.1:9471".parse().unwrap());
    assert!(origin_allowed(&headers, &peer, &Method::POST));
    headers.insert(header::ORIGIN, "https://127.0.0.1:9999".parse().unwrap());
    assert!(!origin_allowed(&headers, &peer, &Method::POST));
    headers.insert(header::ORIGIN, "null".parse().unwrap());
    assert!(!origin_allowed(&headers, &peer, &Method::POST));
    headers.remove(header::ORIGIN);
    headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
    assert!(!origin_allowed(&headers, &peer, &Method::GET));
    headers.insert("sec-fetch-mode", "navigate".parse().unwrap());
    headers.insert("sec-fetch-dest", "document".parse().unwrap());
    assert!(origin_allowed(&headers, &peer, &Method::GET));
    headers.insert(header::HOST, "attacker.example:9471".parse().unwrap());
    assert!(!origin_allowed(&headers, &peer, &Method::GET));
    let standard_https = WebPolicy::new("https://127.0.0.1:443", "TestAccess".into()).unwrap();
    headers.insert(header::HOST, "127.0.0.1".parse().unwrap());
    headers.insert(header::ORIGIN, "https://127.0.0.1".parse().unwrap());
    headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
    assert!(origin_allowed(&headers, &standard_https, &Method::POST));
    headers.insert(header::HOST, "127.0.0.1:9471".parse().unwrap());
    assert!(!origin_allowed(&headers, &standard_https, &Method::POST));
}

#[test]
fn browser_policy_requires_a_fixed_http_origin_without_extra_url_components() {
    for origin in [
        "ftp://hub.example",
        "https://user@hub.example",
        "https://hub.example/admin/",
        "https://hub.example?key=secret",
        "https://hub.example#key",
        "http://127.0.0.1:0",
    ] {
        assert!(
            WebPolicy::new(origin, "TestAccess".into()).is_err(),
            "{origin}"
        );
    }
    let policy = WebPolicy::new("http://127.0.0.1:9472", "TestAccess".into()).unwrap();
    let mut proof = HeaderMap::new();
    proof.insert(header::HOST, "127.0.0.1:9472".parse().unwrap());
    proof.insert(header::ORIGIN, "http://127.0.0.1:9472".parse().unwrap());
    assert!(origin_allowed(&proof, &policy, &Method::POST));
    for host in ["localhost:9472", "127.0.0.1:9473", "attacker.example:9472"] {
        proof.insert(header::HOST, host.parse().unwrap());
        assert!(!origin_allowed(&proof, &policy, &Method::POST));
    }
}

#[tokio::test]
async fn loopback_http_management_needs_no_tls_and_enforces_browser_boundaries() {
    let fixture = Fixture::new_http().await;
    let landing = fixture.client.get(&fixture.url).send().await.unwrap();
    assert_eq!(landing.status(), StatusCode::SEE_OTHER);
    assert_eq!(landing.headers()[header::LOCATION], "/admin/");
    let csrf = fixture.csrf().await;
    let request = || {
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"command":"hub_network_snapshot","args":{}}))
    };
    let response = request().send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    let revision = response["result"]["revision"].clone();
    assert_eq!(fixture.post("/admin/command")
        .header("x-moyai-csrf", &csrf)
        .json(&json!({"command":"hub_network_group","args":{"expectedRevision":revision,"id":null,"label":"Local browser group"}}))
        .send().await.unwrap().status(), StatusCode::OK);
    let local = fixture
        .admin
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(local["groups"][0]["label"], "Local browser group");
    for (name, value) in [
        ("origin", "https://attacker.example"),
        ("host", "attacker.example"),
        ("x-moyai-csrf", "wrong"),
        ("content-type", "text/plain"),
    ] {
        let mut denied = request().build().unwrap();
        denied.headers_mut().insert(
            header::HeaderName::from_static(name),
            value.parse().unwrap(),
        );
        assert_eq!(
            fixture.client.execute(denied).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "{name}"
        );
    }
    let mut forwarded = request().build().unwrap();
    forwarded
        .headers_mut()
        .insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
    forwarded.headers_mut().insert(
        "forwarded",
        format!("proto=http;host={}", fixture.policy.authority)
            .parse()
            .unwrap(),
    );
    forwarded.headers_mut().insert(
        "x-forwarded-host",
        fixture.policy.authority.parse().unwrap(),
    );
    forwarded
        .headers_mut()
        .insert("x-forwarded-proto", "http".parse().unwrap());
    assert_eq!(
        fixture.client.execute(forwarded).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    for (name, value) in [
        ("origin", "https://attacker.example"),
        ("x-moyai-csrf", "wrong"),
        ("content-type", "text/plain"),
    ] {
        assert_eq!(
            request().header(name, value).send().await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "duplicate {name}"
        );
    }
    assert_eq!(
        fixture
            .client
            .post(format!("{}/admin/command", fixture.url))
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"command":"hub_snapshot","args":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post("/host/command")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.admin.web_access.disable();
    assert_eq!(
        request().send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

struct Fixture {
    _directory: tempfile::TempDir,
    admin: Arc<Admin>,
    client: reqwest::Client,
    url: String,
    task: tokio::task::JoinHandle<()>,
    policy: WebPolicy,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new_http() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let url = format!("http://{address}");
        admin.web_access.enable().unwrap();
        let policy = WebPolicy::new(&url, admin.web_access.epoch().unwrap()).unwrap();
        let app = router(Arc::downgrade(&admin), policy.clone());
        let task = tokio::spawn(async move {
            axum::serve(tcp, app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        Self {
            _directory: directory,
            admin,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            url,
            task,
            policy,
        }
    }
    async fn new() -> Self {
        use crate::network::{DeviceNetwork, tls};
        let directory = tempfile::tempdir().unwrap();
        let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
        let pki = DeviceNetwork::open(
            &directory.path().join("fixture-tls.json"),
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();
        let (certificate, key) = pki
            .server_certificate("127.0.0.1".parse().unwrap(), crate::service::now_ms())
            .unwrap();
        let ca = pki.ca_certificate().to_owned();
        let acceptor = tls::acceptor(&certificate, &key, &ca, true).unwrap();
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let listener = tls::NetworkListener::new(tcp, acceptor);
        admin.web_access.enable().unwrap();
        let policy = WebPolicy::new(
            &format!("https://{address}"),
            admin.web_access.epoch().unwrap(),
        )
        .unwrap();
        let app = router(Arc::downgrade(&admin), policy.clone());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<TlsPeer>(),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self {
            _directory: directory,
            admin,
            client,
            url: format!("https://{address}"),
            task,
            policy,
        }
    }
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.url))
            .header(header::ORIGIN, &self.url)
    }
    async fn csrf(&self) -> String {
        let response = self
            .client
            .get(format!("{}/admin/session", self.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["epoch"], self.policy.access_id);
        body["csrf"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn abandoned_browser_requests_hold_slots_until_the_actual_dispatch_finishes() {
    let fixture = Fixture::new().await;
    let csrf = fixture.csrf().await;
    let snapshot = fixture
        .admin
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    let revision = snapshot["revision"].clone();
    let mut headers = HeaderMap::new();
    headers.insert("x-moyai-csrf", csrf.parse().unwrap());
    let state = WebState {
        admin: Arc::downgrade(&fixture.admin),
        policy: fixture.policy.clone(),
    };
    let lock = crate::admin::test_support::hold_network(&fixture.admin);
    for _ in 0..8 {
        let operation = command(
            State(state.clone()),
            headers.clone(),
            Json(Command {
                command: "hub_network_group".into(),
                args: json!({"expectedRevision": revision, "id": null, "label": "One admitted update"}),
            }),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(5), operation)
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.admin.web_access.slots.available_permits(), 0);
    assert_eq!(
        command(
            State(state),
            headers,
            Json(Command {
                command: "hub_snapshot".into(),
                args: json!({}),
            })
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(lock);
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.admin.web_access.slots.available_permits() != 8 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let final_snapshot = fixture
        .admin
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(final_snapshot["groups"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn tls_browser_management_uses_shared_revision_owner_and_rejects_local_only_commands() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .client
            .get(format!("{}/admin/", fixture.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let csrf = fixture.csrf().await;
    let request = |command: &str, args: Value| {
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"command": command, "args": args}))
    };
    let missing_csrf = fixture
        .post("/admin/command")
        .json(&json!({"command":"hub_snapshot","args":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    let response = request("hub_network_snapshot", json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert!(
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    let snapshot: Value = response.json().await.unwrap();
    let revision = snapshot["result"]["revision"].clone();
    let change =
        json!({"expectedRevision": revision, "id": null, "label": "Browser managed group"});
    assert_eq!(
        request("hub_network_group", change.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request("hub_network_group", change)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let local = fixture
        .admin
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert!(
        local["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["label"] == "Browser managed group")
    );
    for local_only in [
        "hub_host_shutdown",
        "hub_web_enable",
        "hub_web_disable",
        "hub_history_export",
    ] {
        assert_eq!(
            request(local_only, json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        fixture
            .post("/admin/command")
            .json(&json!({"command":"hub_snapshot","args":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for obsolete in ["/admin/login", "/admin/login.js", "/admin/logout"] {
        assert_eq!(
            fixture
                .client
                .get(format!("{}{obsolete}", fixture.url))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for command in ["hub_network_stop", "hub_stop", "hub_web_status"] {
        assert_eq!(
            request(command, json!({})).send().await.unwrap().status(),
            StatusCode::OK
        );
    }
    for command in ["hub_network_start", "hub_start"] {
        // A missing required field reaches the shared command validator. The
        // browser must not reject these server controls as native-only actions.
        assert_eq!(
            request(command, json!({})).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
}
