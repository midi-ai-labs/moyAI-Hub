use super::*;

#[test]
fn owned_https_file_relocation_respects_components_and_external_paths() {
    let (source, destination) = if cfg!(windows) {
        (
            PathBuf::from("C:/Users/test/old-hub"),
            PathBuf::from("C:/Users/test/new-hub"),
        )
    } else {
        (PathBuf::from("/tmp/old-hub"), PathBuf::from("/tmp/new-hub"))
    };
    assert_eq!(
        owned_path_relocation(&source.join("tls/cert.pem"), &source, &destination),
        Some(destination.join("tls/cert.pem"))
    );
    assert!(
        owned_path_relocation(
            &source.with_file_name("old-hub-other").join("key.pem"),
            &source,
            &destination
        )
        .is_none()
    );
    assert!(
        owned_path_relocation(&source.join("../workspace/key.pem"), &source, &destination)
            .is_none()
    );
    assert!(owned_path_relocation(Path::new("relative/key.pem"), &source, &destination).is_none());
    #[cfg(windows)]
    assert_eq!(
        owned_path_relocation(
            Path::new(r"\\?\C:\USERS\TEST\OLD-HUB\tls\key.pem"),
            &source,
            &destination
        ),
        Some(destination.join("tls/key.pem"))
    );
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}
async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn open(directory: &tempfile::TempDir) -> Arc<Admin> {
    Admin::open(directory.path().join("catalog.json")).unwrap()
}
async fn status(admin: &Arc<Admin>) -> Value {
    admin.dispatch("hub_web_status", json!({})).await.unwrap()
}
async fn enable(admin: &Arc<Admin>, settings: WebSettings) -> Result<Value, String> {
    admin
        .dispatch("hub_web_enable", json!({"settings":settings}))
        .await
}

#[tokio::test]
async fn local_management_is_independent_and_settings_restore_without_access() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    assert_eq!(
        status(&admin).await["settings"],
        json!({"local_port":0,"remote":{"mode":"off"}})
    );
    let started = enable(&admin, WebSettings::default()).await.unwrap();
    let url = started["local_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(url.ends_with("/admin/"));
    assert_eq!(started["enabled"], true);
    assert!(started.get("key").is_none());
    assert!(started.get("key_id").is_none());
    assert_eq!(started["remote_url"], Value::Null);
    assert_eq!(client().get(&url).send().await.unwrap().status(), 200);
    let local_session: Value = client()
        .get(format!("{}session", url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(local_session["authentication"]["login_allowed"], true);
    assert_eq!(local_session["authentication"]["bootstrap_allowed"], false);
    assert!(enable(&admin, WebSettings::default()).await.is_err());
    admin.dispatch("hub_network_stop", json!({})).await.unwrap();
    assert_eq!(client().get(&url).send().await.unwrap().status(), 200);
    assert_eq!(status(&admin).await["epoch"], started["epoch"]);
    let saved: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("web-management.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["schema"], 1);
    assert!(saved.get("key").is_none());
    admin.shutdown().await.unwrap();
    assert!(!admin.web_access.enabled());
    assert!(client().get(&url).send().await.is_err());
    drop(admin);
    let restored = open(&directory);
    let state = status(&restored).await;
    assert_eq!(state["settings"], saved["settings"]);
    assert_eq!(state["enabled"], false);
    assert_eq!(state["epoch"], Value::Null);
    assert_eq!(state["local_url"], Value::Null);
}

#[tokio::test]
async fn proxy_listener_uses_fixed_public_origin_and_survives_network_stop() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    let port = free_port().await;
    let settings = WebSettings {
        local_port: 0,
        remote: Remote::Proxy {
            bind: format!("127.0.0.1:{port}"),
            public_origin: "https://MANAGEMENT.example:443/".into(),
        },
    };
    let started = enable(&admin, settings).await.unwrap();
    assert_eq!(started["remote_url"], "https://management.example/admin/");
    let url = format!("http://127.0.0.1:{port}/admin/");
    assert_eq!(
        client()
            .get(&url)
            .header("host", "management.example")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client()
            .get(&url)
            .header("host", "other.example")
            .header("x-forwarded-host", "management.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    admin.dispatch("hub_network_stop", json!({})).await.unwrap();
    assert_eq!(
        client()
            .get(&url)
            .header("host", "management.example")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    admin.dispatch("hub_web_disable", json!({})).await.unwrap();
    assert!(client().get(&url).send().await.is_err());
    let restarted = admin.dispatch("hub_web_enable", json!({})).await.unwrap();
    assert_ne!(started["epoch"], restarted["epoch"]);
    assert_eq!(restarted["remote_url"], started["remote_url"]);
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_http_listener_serves_guidance_but_requires_safe_transport_for_management() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    let port = free_port().await;
    let authority = format!("hub.lan:{port}");
    let origin = format!("http://{authority}");
    let started = enable(
        &admin,
        WebSettings {
            local_port: 0,
            remote: Remote::Http {
                bind: format!("0.0.0.0:{port}"),
                public_origin: origin.clone(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(started["remote_url"], format!("{origin}/admin/"));
    let url = format!("http://127.0.0.1:{port}/admin/");
    let response = client()
        .get(format!("{url}session"))
        .header("host", &authority)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("set-cookie").is_none());
    let session: Value = response.json().await.unwrap();
    assert_eq!(
        session["authentication"],
        json!({
            "configured":false,"user":null,"session_id":null,
            "bootstrap_allowed":false,"login_allowed":false,"local_management":false
        })
    );
    let csrf = session["csrf"].as_str().unwrap();
    let command = || {
        client()
            .post(format!("{url}command"))
            .header("host", &authority)
            .header("origin", &origin)
            .header("x-moyai-csrf", csrf)
            .json(&json!({"command":"hub_network_snapshot","args":{}}))
    };
    assert_eq!(command().send().await.unwrap().status(), 403);
    assert_eq!(
        client()
            .get(&url)
            .header("host", &authority)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let username = "http-listener-admin";
    let password = "Http listener administrator password";
    let token = {
        use crate::identity::{self, SessionAudience};
        let store = admin.shared_work.lock().unwrap();
        let db = store.identity_connection();
        let now = crate::service::now_ms();
        identity::bootstrap_admin(db, username, "Listener administrator", password, now).unwrap();
        identity::login(db, username, password, SessionAudience::WebAdmin, None, now)
            .unwrap()
            .token
    };
    let cookie = format!("moyai_admin={token}");
    let request_session = crate::web_admin::authentication::session_id(&token);
    assert_eq!(
        command()
            .header("cookie", &cookie)
            .header("x-moyai-session", &request_session)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .post(format!("{url}access"))
            .header("host", &authority)
            .header("origin", &origin)
            .header("x-moyai-csrf", csrf)
            .json(&json!({"ticket":"a".repeat(64)}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let protected: Value = client()
        .get(format!("{url}session"))
        .header("host", &authority)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(protected["authentication"]["configured"], true);
    assert_eq!(protected["authentication"]["login_allowed"], false);
    assert!(protected["authentication"]["user"].is_null());
    // The independent direct-loopback listener remains usable with human authentication.
    let local = started["local_url"].as_str().unwrap();
    let local_origin = reqwest::Url::parse(local)
        .unwrap()
        .origin()
        .ascii_serialization();
    let local_session: Value = client()
        .get(format!("{local}session"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(local_session["authentication"]["login_allowed"], true);
    let local_command = || {
        client()
            .post(format!("{local}command"))
            .header("origin", &local_origin)
            .header("x-moyai-csrf", local_session["csrf"].as_str().unwrap())
            .json(&json!({"command":"hub_network_snapshot","args":{}}))
    };
    assert_eq!(local_command().send().await.unwrap().status(), 401);
    assert_eq!(
        local_command()
            .header("cookie", &cookie)
            .header("x-moyai-session", &request_session)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(client().get(&url).send().await.unwrap().status(), 403);
    let mut forged = command().build().unwrap();
    forged
        .headers_mut()
        .insert("origin", "http://attacker.lan".parse().unwrap());
    assert_eq!(client().execute(forged).await.unwrap().status(), 403);
    admin.dispatch("hub_web_disable", json!({})).await.unwrap();
    let restarted = admin.dispatch("hub_web_enable", json!({})).await.unwrap();
    assert_ne!(started["epoch"], restarted["epoch"]);
    assert_eq!(command().send().await.unwrap().status(), 403);
    admin.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_bind_and_failed_save_retire_all_new_sockets_and_epochs() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = free_port().await;
    let settings = WebSettings {
        local_port,
        remote: Remote::Proxy {
            bind: occupied.local_addr().unwrap().to_string(),
            public_origin: "https://admin.example".into(),
        },
    };
    assert!(enable(&admin, settings).await.is_err());
    assert_eq!(status(&admin).await["enabled"], false);
    assert!(!directory.path().join("web-management.json").exists());
    let released = TcpListener::bind((Ipv4Addr::LOCALHOST, local_port))
        .await
        .unwrap();
    drop(released);
    let original = enable(&admin, WebSettings::default()).await.unwrap();
    admin.dispatch("hub_web_disable", json!({})).await.unwrap();
    let persisted = std::fs::read(directory.path().join("web-management.json")).unwrap();
    let impossible = directory.path().join("destination-is-a-directory");
    std::fs::create_dir(&impossible).unwrap();
    admin.web_server.lock().await.path = impossible;
    assert!(
        enable(
            &admin,
            WebSettings {
                local_port,
                remote: Remote::Off
            }
        )
        .await
        .is_err()
    );
    let failed = status(&admin).await;
    assert_eq!(failed["enabled"], false);
    assert_eq!(failed["epoch"], Value::Null);
    assert_eq!(failed["settings"], original["settings"]);
    assert_eq!(
        std::fs::read(directory.path().join("web-management.json")).unwrap(),
        persisted
    );
    TcpListener::bind((Ipv4Addr::LOCALHOST, local_port))
        .await
        .unwrap();
}

#[tokio::test]
async fn concurrent_enable_and_cancelled_stop_keep_one_lifecycle_owner() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    let (first, second) = tokio::join!(
        enable(&admin, WebSettings::default()),
        enable(&admin, WebSettings::default())
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let started = first.or(second).unwrap();
    let mut stopped = Box::pin(controls(&admin, "hub_web_disable", json!({})));
    // Polling dispatch transfers the operation into an owned task. Dropping this
    // caller future cannot drop that task or its eventual listener teardown.
    assert!(futures_util::poll!(stopped.as_mut()).is_pending());
    drop(stopped);
    tokio::time::timeout(Duration::from_secs(3), async {
        while status(&admin).await["enabled"] == true {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status(&admin).await["epoch"], Value::Null);
    assert!(
        client()
            .get(started["local_url"].as_str().unwrap())
            .send()
            .await
            .is_err()
    );
}

fn certificate(directory: &Path, names: Vec<String>, expired: bool) -> (PathBuf, PathBuf, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(names).unwrap();
    if expired {
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(2021, 1, 1);
    }
    let cert = params.self_signed(&key).unwrap().pem();
    let cert_path = directory.join("external-cert.pem");
    let key_path = directory.join("external-key.pem");
    std::fs::write(&cert_path, &cert).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    (cert_path, key_path, cert)
}
#[tokio::test]
async fn external_https_requires_matching_current_certificate_but_no_client_certificate() {
    let directory = tempfile::tempdir().unwrap();
    let admin = open(&directory);
    let port = free_port().await;
    let (certificate_path, private_key_path, cert) =
        certificate(directory.path(), vec!["127.0.0.1".into()], false);
    let settings = WebSettings {
        local_port: 0,
        remote: Remote::Https {
            bind: format!("127.0.0.1:{port}"),
            public_origin: format!("https://127.0.0.1:{port}"),
            certificate_path: certificate_path.clone(),
            private_key_path: private_key_path.clone(),
        },
    };
    let started = enable(&admin, settings).await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .add_root_certificate(reqwest::Certificate::from_pem(cert.as_bytes()).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(started["remote_url"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(
        https_acceptor(
            &certificate_path,
            &private_key_path,
            "https://wrong.example"
        )
        .is_err()
    );
    admin.shutdown().await.unwrap();
    let (certificate_path, private_key_path, _) =
        certificate(directory.path(), vec!["127.0.0.1".into()], true);
    assert!(https_acceptor(&certificate_path, &private_key_path, "https://127.0.0.1").is_err());
}

#[test]
fn persisted_web_settings_are_strict_and_do_not_read_certificates_until_start() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("web-management.json");
    let settings = WebSettings {
        local_port: 0,
        remote: Remote::Https {
            bind: "0.0.0.0:9443".into(),
            public_origin: "https://admin.example".into(),
            certificate_path: directory.path().join("not-installed-cert.pem"),
            private_key_path: directory.path().join("not-installed-key.pem"),
        },
    };
    save(&file, settings.clone()).unwrap();
    assert_eq!(Manager::open(file.clone()).settings, settings);
    assert!(
        WebSettings {
            local_port: 0,
            remote: Remote::Proxy {
                bind: "0.0.0.0:9443".into(),
                public_origin: "https://admin.example".into()
            }
        }
        .validated()
        .is_err()
    );
    assert!(
        WebSettings {
            local_port: 0,
            remote: Remote::Proxy {
                bind: "127.0.0.1:9443".into(),
                public_origin: "http://admin.example".into()
            }
        }
        .validated()
        .is_err()
    );
    assert!(canonical_origin("https://admin.example/admin/", "https").is_err());
    std::fs::write(
        &file,
        br#"{"schema":2,"settings":{"local_port":0,"remote":{"mode":"off"}}}"#,
    )
    .unwrap();
    assert!(Manager::open(file).settings_error.is_some());
}

#[tokio::test]
async fn corrupt_web_settings_leave_hub_available_until_explicit_replacement_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("web-management.json");
    let damaged = b"{not a valid settings document";
    std::fs::write(&file, damaged).unwrap();
    let admin = open(&directory);
    admin.dispatch("hub_snapshot", json!({})).await.unwrap();
    let state = status(&admin).await;
    assert_eq!(state["enabled"], false);
    assert!(state["settings_error"].is_string());
    assert_eq!(
        state["settings"],
        json!({"local_port":0,"remote":{"mode":"off"}})
    );
    assert!(admin.dispatch("hub_web_enable", json!({})).await.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), damaged);
    let started = enable(&admin, WebSettings::default()).await.unwrap();
    assert_eq!(started["settings_error"], Value::Null);
    assert_eq!(started["enabled"], true);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&file).unwrap()).unwrap()["schema"],
        1
    );
    admin.shutdown().await.unwrap();
}
