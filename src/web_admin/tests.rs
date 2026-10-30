use super::*;

#[tokio::test]
async fn shared_management_http_uses_cookie_principal_cas_and_exact_retry() {
    let fixture = Fixture::new_http().await;
    let csrf = fixture.csrf().await;
    let send = |command: &str, args: Value| {
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"command":command,"args":args}))
    };
    let snapshot = send("hub_shared_snapshot", json!({})).send().await.unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = snapshot.json().await.unwrap();
    let args = json!({"request_id":"browser-group","expected_revision":snapshot["result"]["revision"],"request":{"kind":"create_group","id":"human-team","label":"人のチーム"}});
    let response = send("hub_shared_command", args.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["groups"][0]["id"], "human-team");
    let repeated = send("hub_shared_command", args.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::OK);
    let mut changed = args.clone();
    changed["request"]["label"] = json!("Different");
    let conflict = send("hub_shared_command", changed).send().await.unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(conflict.json::<Value>().await.unwrap()["code"], "conflict");
    let mut stale = args.clone();
    stale["request_id"] = json!("new-but-stale");
    stale["request"]["id"] = json!("another");
    assert_eq!(
        send("hub_shared_command", stale)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    // The existing network-admin dispatch cannot accept an invented management actor.
    assert!(
        fixture
            .admin
            .dispatch("hub_shared_command", args)
            .await
            .is_err()
    );
    let anonymous = reqwest::Client::new()
        .post(format!("{}/admin/command", fixture.url))
        .header(header::ORIGIN, &fixture.url)
        .header("x-moyai-csrf", &csrf)
        .json(&json!({"command":"hub_shared_snapshot","args":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

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
    cookie: Option<String>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new_http() -> Self {
        Self::http(true).await
    }
    async fn unconfigured_http() -> Self {
        Self::http(false).await
    }
    async fn http(authenticated: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
        let cookie = authenticated.then(|| administrator_cookie(&admin));
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let url = format!("http://{address}");
        admin.web_access.enable().unwrap();
        let policy = WebPolicy::new(&url, admin.web_access.epoch().unwrap())
            .unwrap()
            .local_management();
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
                .default_headers(cookie_headers(cookie.as_deref()))
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            url,
            task,
            policy,
            cookie,
        }
    }
    async fn new() -> Self {
        use crate::network::{DeviceNetwork, tls};
        let directory = tempfile::tempdir().unwrap();
        let admin = Admin::open(directory.path().join("catalog.json")).unwrap();
        let cookie = administrator_cookie(&admin);
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
            .default_headers(cookie_headers(Some(&cookie)))
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
            cookie: Some(cookie),
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

const ADMIN_USERNAME: &str = "fixture-admin";
const ADMIN_PASSWORD: &str = "Fixture administrator password 2026";

fn cookie_headers(cookie: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(cookie) = cookie {
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        headers.insert("x-moyai-session", cookie_session(cookie).parse().unwrap());
    }
    headers
}

fn cookie_session(cookie: &str) -> String {
    authentication::session_id(cookie.strip_prefix("moyai_admin=").unwrap())
}

fn administrator_cookie(admin: &Admin) -> String {
    use crate::identity::{self, SessionAudience};
    let store = admin.shared_work.lock().unwrap();
    let db = store.identity_connection();
    let now = crate::service::now_ms();
    identity::bootstrap_admin(
        db,
        ADMIN_USERNAME,
        "Fixture administrator",
        ADMIN_PASSWORD,
        now,
    )
    .unwrap();
    let session = identity::login(
        db,
        ADMIN_USERNAME,
        ADMIN_PASSWORD,
        SessionAudience::WebAdmin,
        None,
        now,
    )
    .unwrap();
    format!("moyai_admin={}", session.token)
}

fn response_cookie(response: &reqwest::Response, secure: bool) -> String {
    let value = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let attributes: Vec<_> = value.split(';').map(str::trim).collect();
    assert!(attributes.contains(&"HttpOnly"));
    assert!(attributes.contains(&"SameSite=Strict"));
    assert!(attributes.contains(&"Path=/admin"));
    if !attributes[0].ends_with('=') {
        assert!(attributes.contains(&"Max-Age=34560000"));
    }
    assert_eq!(attributes.contains(&"Secure"), secure);
    assert!(attributes[0].starts_with("moyai_admin="));
    attributes[0].to_owned()
}

fn access_payload(fixture: &Fixture) -> Value {
    let user = {
        let store = fixture.admin.shared_work.lock().unwrap();
        crate::identity::web_access::local_owner(
            store.identity_connection(),
            crate::service::now_ms(),
        )
        .unwrap()
    };
    let url = fixture
        .admin
        .web_access
        .issue_ticket(&fixture.url, user.user_id, None)
        .unwrap();
    json!({"ticket":url.split_once("#access=").unwrap().1})
}

#[tokio::test]
async fn browser_access_requires_host_ticket_origin_csrf_and_single_use() {
    let fixture = Fixture::unconfigured_http().await;
    let csrf = fixture.csrf().await;
    let payload = access_payload(&fixture);
    assert_eq!(
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"command":"hub_snapshot","args":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post("/admin/access")
            .json(&payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post("/admin/access")
            .header("x-moyai-csrf", &csrf)
            .header("x-forwarded-for", "127.0.0.1")
            .json(&payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post("/admin/access")
            .header("x-moyai-csrf", &csrf)
            .json(&json!({"ticket":"bad"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for retired in ["/admin/login", "/admin/bootstrap"] {
        assert_eq!(
            fixture
                .post(retired)
                .header("x-moyai-csrf", &csrf)
                .json(&json!({"username":"x","password":"x"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let response = fixture
        .post("/admin/access")
        .header("x-moyai-csrf", &csrf)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response_cookie(&response, false);
    let signed: Value = response.json().await.unwrap();
    assert_eq!(signed["authentication"]["user"]["administrator"], true);
    assert_eq!(signed["authentication"]["bootstrap_allowed"], false);
    assert!(signed.get("token").is_none());
    let session = signed["authentication"]["session_id"].as_str().unwrap();
    let command = |sid: &str| {
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .header("cookie", &cookie)
            .header("x-moyai-session", sid)
            .json(&json!({"command":"hub_snapshot","args":{}}))
    };
    assert_eq!(
        command(session).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        command("stale-document").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post("/admin/access")
            .header("x-moyai-csrf", &csrf)
            .json(&payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // A browser cannot ask the trusted native Host for another ticket.
    assert_eq!(
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .header("cookie", &cookie)
            .header("x-moyai-session", session)
            .json(&json!({"command":"hub_web_open","args":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = fixture
        .post("/admin/logout")
        .header("x-moyai-csrf", &csrf)
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_cookie(&response, false), "moyai_admin=");
    assert_eq!(
        command(session).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let reopened = fixture
        .post("/admin/access")
        .header("x-moyai-csrf", &csrf)
        .json(&access_payload(&fixture))
        .send()
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::OK);
    let next: Value = reopened.json().await.unwrap();
    assert_eq!(
        next["authentication"]["user"],
        signed["authentication"]["user"]
    );
    assert_ne!(
        next["authentication"]["session_id"],
        signed["authentication"]["session_id"]
    );
}

#[tokio::test]
async fn https_administrator_access_sets_secure_cookie_and_rechecks_current_role() {
    let fixture = Fixture::new().await;
    let csrf = fixture.csrf().await;
    let payload = access_payload(&fixture);
    let response = fixture
        .post("/admin/access")
        .header("x-moyai-csrf", &csrf)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response_cookie(&response, true);
    let session: Value = response.json().await.unwrap();
    let user = session["authentication"]["user"]["user_id"]
        .as_str()
        .unwrap();
    {
        let store = fixture.admin.shared_work.lock().unwrap();
        store
            .identity_connection()
            .execute(
                "UPDATE identity_users SET administrator=0 WHERE id=?1",
                [user],
            )
            .unwrap();
    }
    assert_eq!(
        fixture
            .post("/admin/command")
            .header("x-moyai-csrf", &csrf)
            .header("cookie", &cookie)
            .header(
                "x-moyai-session",
                session["authentication"]["session_id"].as_str().unwrap()
            )
            .json(&json!({"command":"hub_snapshot","args":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[test]
fn browser_ticket_is_origin_epoch_expiry_bound_and_single_use() {
    let access = WebAccess::default();
    access.enable().unwrap();
    let local = WebPolicy::new("http://127.0.0.1:1234", access.epoch().unwrap()).unwrap();
    let other = WebPolicy::new("https://other.example", access.epoch().unwrap()).unwrap();
    let url = access
        .issue_ticket(&local.origin, "actor".into(), None)
        .unwrap();
    let token = url.split_once("#access=").unwrap().1;
    assert!(access.consume_ticket(token, &other).is_err());
    assert!(access.consume_ticket(token, &local).is_ok());
    assert!(access.consume_ticket(token, &local).is_err());
    let url = access
        .issue_ticket(&local.origin, "actor".into(), None)
        .unwrap();
    let token = url.split_once("#access=").unwrap().1;
    access.state.lock().unwrap().tickets[0].expires =
        std::time::Instant::now() - Duration::from_secs(1);
    assert!(access.consume_ticket(token, &local).is_err());
    let url = access
        .issue_ticket(&local.origin, "actor".into(), None)
        .unwrap();
    access.enable().unwrap();
    assert!(
        access
            .consume_ticket(url.split_once("#access=").unwrap().1, &local)
            .is_err()
    );
}

#[tokio::test]
async fn session_reads_wait_off_executor_and_keep_a_bounded_slot_after_disconnect() {
    let fixture = Fixture::unconfigured_http().await;
    let state = WebState {
        admin: Arc::downgrade(&fixture.admin),
        policy: fixture.policy.clone(),
    };
    let headers = HeaderMap::new();
    let owner = fixture.admin.shared_work.lock().unwrap();
    for _ in 0..8 {
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                authentication::session_response(&state, &headers, true, None)
            )
            .await
            .is_err()
        );
    }
    assert_eq!(
        authentication::session_response(&state, &headers, true, None)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(owner);
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.admin.web_access.read_slots.available_permits() != 8 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        authentication::session_response(&state, &headers, true, None)
            .await
            .status(),
        StatusCode::OK
    );
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
    let mut headers = cookie_headers(fixture.cookie.as_deref());
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
            axum::Extension(authentication::BootstrapAccess(true)),
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
            axum::Extension(authentication::BootstrapAccess(true)),
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
        "hub_autostart_status",
        "hub_autostart_set",
        "hub_firewall_preview",
        "hub_firewall_status",
        "hub_firewall_retired",
        "hub_firewall_remove_retired",
        "hub_firewall_export",
        "hub_firewall_apply",
        "hub_firewall_remove",
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
    for obsolete in ["/admin/login.js"] {
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
    for mutation in ["/admin/access", "/admin/logout"] {
        assert_eq!(
            fixture
                .client
                .get(format!("{}{mutation}", fixture.url))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for command in ["hub_network_stop", "hub_stop", "hub_web_status"] {
        assert_eq!(
            request(command, json!({})).send().await.unwrap().status(),
            if command == "hub_web_status" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    for command in ["hub_network_start", "hub_start"] {
        // Host lifecycle is changed from the Hub PC's native-opened browser.
        assert_eq!(
            request(command, json!({})).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn queued_network_mutation_reauthenticates_remote_device_after_owner_wait() {
    let fixture = Fixture::new_http().await;
    let now = crate::service::now_ms();
    let (device_id, remote_token, revision) = {
        let store = fixture.admin.shared_work.lock().unwrap();
        let actor = crate::identity::web_access::local_owner(&store.db, now).unwrap();
        let mut network = fixture.admin.network.lock().unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let csr = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
            .unwrap()
            .serialize_request(&key)
            .unwrap()
            .pem()
            .unwrap();
        let invitation_revision = network.revision();
        let invitation = network
            .invite(
                &invitation_revision,
                "Remote admin PC".into(),
                Default::default(),
                now,
            )
            .unwrap();
        let receipt = network
            .enroll(
                invitation["code"].as_str().unwrap(),
                &csr,
                "127.0.0.1".parse().unwrap(),
                now,
            )
            .unwrap();
        crate::identity::device::bind(
            &store.db,
            &actor,
            &receipt.device_id,
            Some(&actor.user_id),
            "Remote admin PC",
            now,
        )
        .unwrap();
        let proof = crate::identity::web_access::device_proof(
            &store.db,
            &actor.user_id,
            &receipt.device_id,
            &receipt.certificate_sha256,
        )
        .unwrap();
        let session =
            crate::identity::web_access::issue(&store.db, &actor.user_id, Some(&proof), now)
                .unwrap();
        (receipt.device_id, session.token, network.revision())
    };
    let mut network = fixture.admin.network.lock().unwrap();
    let permit = fixture
        .admin
        .web_access
        .slots
        .clone()
        .try_acquire_owned()
        .unwrap();
    let mut operation = tokio::spawn(shared::command(
        fixture.admin.clone(),
        Command {
            command: "hub_network_group".into(),
            args: json!({"expectedRevision":revision,"id":null,"label":"Must not be created"}),
        },
        remote_token,
        permit,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut operation)
            .await
            .is_err()
    );
    let revoke_revision = network.revision();
    network
        .set_device(
            &revoke_revision,
            &device_id,
            "Remote admin PC".into(),
            Default::default(),
            true,
        )
        .unwrap();
    drop(network);
    let response = tokio::time::timeout(Duration::from_secs(3), operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        fixture.admin.network.lock().unwrap().snapshot(now)["groups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn remote_cookie_on_loopback_keeps_project_management_but_cannot_change_host_settings() {
    let fixture = Fixture::new_http().await;
    let now = crate::service::now_ms();
    let (device, token) = {
        let store = fixture.admin.shared_work.lock().unwrap();
        let actor = crate::identity::web_access::local_owner(&store.db, now).unwrap();
        let mut network = fixture.admin.network.lock().unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let csr = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
            .unwrap()
            .serialize_request(&key)
            .unwrap()
            .pem()
            .unwrap();
        let revision = network.revision();
        let invitation = network
            .invite(&revision, "Remote admin PC".into(), Default::default(), now)
            .unwrap();
        let receipt = network
            .enroll(
                invitation["code"].as_str().unwrap(),
                &csr,
                "127.0.0.1".parse().unwrap(),
                now,
            )
            .unwrap();
        crate::identity::device::bind(
            &store.db,
            &actor,
            &receipt.device_id,
            Some(&actor.user_id),
            "Remote admin PC",
            now,
        )
        .unwrap();
        let proof = crate::identity::web_access::device_proof(
            &store.db,
            &actor.user_id,
            &receipt.device_id,
            &receipt.certificate_sha256,
        )
        .unwrap();
        let session =
            crate::identity::web_access::issue(&store.db, &actor.user_id, Some(&proof), now)
                .unwrap();
        (receipt.device_id, session.token)
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let cookie = format!("moyai_admin={token}");
    let session = client
        .get(format!("{}/admin/session", fixture.url))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);
    let session: Value = session.json().await.unwrap();
    assert_eq!(session["authentication"]["local_management"], false);
    let request = |command: &str, args: Value| {
        client
            .post(format!("{}/admin/command", fixture.url))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &fixture.url)
            .header("x-moyai-csrf", session["csrf"].as_str().unwrap())
            .header(
                "x-moyai-session",
                session["authentication"]["session_id"].as_str().unwrap(),
            )
            .json(&json!({"command":command,"args":args}))
    };
    let denied = request(
        "hub_set_standard_model",
        json!({"revision":"1","modelId":null}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        denied.json::<Value>().await.unwrap()["code"],
        "requires_hub_pc"
    );
    let denied_roles = request(
        "hub_set_standard_models",
        json!({"revision":"1","mainModelId":null,"sideModelId":null,"approveModelId":null}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(denied_roles.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        denied_roles.json::<Value>().await.unwrap()["code"],
        "requires_hub_pc"
    );
    let snapshot = request("hub_shared_snapshot", json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = snapshot.json().await.unwrap();
    let saved = request("hub_shared_command",json!({"request_id":"remote-project","expected_revision":snapshot["result"]["revision"],"request":{
        "kind":"save_project","id":"remote-project","label":"Remote managed project","archived":false,"controller_device_ids":[device],"runner_device_ids":[]
    }})).send().await.unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved: Value = saved.json().await.unwrap();
    assert!(
        saved["result"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "remote-project")
    );
}
