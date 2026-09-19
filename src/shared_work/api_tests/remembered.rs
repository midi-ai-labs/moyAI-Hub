use super::*;

#[tokio::test]
async fn remembered_login_refresh_logout_and_revocation_use_real_device_tls() {
    let directory = tempfile::tempdir().unwrap();
    let now = crate::service::now_ms();
    let mut network =
        DeviceNetwork::open(&directory.path().join("network.json"), "human-refresh-test").unwrap();
    let first = device(&mut network, "WinA");
    let second = device(&mut network, "WinB");
    let ca = network.ca_certificate().to_owned();
    let (certificate, key) = network
        .server_certificate("127.0.0.1".parse().unwrap(), now)
        .unwrap();
    let work = SharedWork::open(&directory.path().join("work.sqlite3")).unwrap();
    let admin =
        identity::bootstrap_admin(&work.db, "admin", "Admin", "test-password-12345", now).unwrap();
    let alice = identity::create_user(
        &work.db,
        &admin,
        "alice",
        "Alice",
        "test-password-12345",
        false,
        now,
    )
    .unwrap();
    let store = Arc::new(Mutex::new(work));
    let network = Arc::new(Mutex::new(network));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("https://{address}/v1/shared");
    let listener = network::tls::NetworkListener::new(
        listener,
        network::tls::acceptor(&certificate, &key, &ca, true).unwrap(),
    )
    .with_authority(address.to_string());
    let app = router(store.clone(), network.clone());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<network::tls::TlsPeer>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let a = client(&ca, Some(&first));
    let b = client(&ca, Some(&second));
    let anonymous = client(&ca, None);
    let session = body(
        a.post(format!("{endpoint}/login"))
            .json(&json!({"username":"alice","password":"test-password-12345"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let renewal = json!({"refresh_token":session["refresh_token"]});
    for client in [&anonymous, &b] {
        assert_eq!(
            client
                .post(format!("{endpoint}/refresh"))
                .json(&renewal)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    store
        .lock()
        .unwrap()
        .db
        .execute("UPDATE identity_sessions SET expires_at_ms=0", [])
        .unwrap();
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(session["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let fresh = body(
        a.post(format!("{endpoint}/refresh"))
            .json(&renewal)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(fresh["principal"]["user_id"], alice.user_id);
    body(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(fresh["token"].as_str().unwrap())
            .send()
            .await
            .unwrap(),
    )
    .await;
    body(
        a.post(format!("{endpoint}/logout"))
            .json(&renewal)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        a.post(format!("{endpoint}/refresh"))
            .json(&renewal)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(fresh["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let session = body(
        a.post(format!("{endpoint}/login"))
            .json(&json!({"username":"alice","password":"test-password-12345"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let renewal = json!({"refresh_token":session["refresh_token"]});
    {
        let work = store.lock().unwrap();
        identity::update_user(
            &work.db,
            &admin,
            &alice.user_id,
            "Alice",
            false,
            true,
            now + 1,
        )
        .unwrap();
    }
    assert_eq!(
        a.post(format!("{endpoint}/refresh"))
            .json(&renewal)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    {
        let work = store.lock().unwrap();
        identity::update_user(
            &work.db,
            &admin,
            &alice.user_id,
            "Alice",
            false,
            false,
            now + 2,
        )
        .unwrap();
    }
    let session = body(
        a.post(format!("{endpoint}/login"))
            .json(&json!({"username":"alice","password":"test-password-12345"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    {
        let mut network = network.lock().unwrap();
        let revision = network.revision().to_owned();
        network
            .set_device(&revision, &first.id, "WinA".into(), BTreeSet::new(), true)
            .unwrap();
    }
    assert_eq!(
        a.post(format!("{endpoint}/refresh"))
            .json(&json!({"refresh_token":session["refresh_token"]}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}
