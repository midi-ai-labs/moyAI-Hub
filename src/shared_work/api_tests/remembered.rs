use super::*;

#[tokio::test]
async fn passwordless_session_binding_expiry_logout_and_revocation_use_real_device_tls() {
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
    assert_eq!(
        anonymous
            .post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    {
        let work = store.lock().unwrap();
        identity::device::bind(
            &work.db,
            &admin,
            &first.id,
            Some(&alice.user_id),
            "WinA",
            now,
        )
        .unwrap();
        identity::device::enroll(&work.db, &second.id, "WinB", now).unwrap();
    }
    for route in ["login", "setup-password", "refresh"] {
        assert_eq!(
            a.post(format!("{endpoint}/{route}"))
                .json(&json!({"username":"alice","password":"test-password-12345"}))
                .send()
                .await
                .unwrap()
                .status(),
            410
        );
    }
    let session = body(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let token = session["token"].as_str().unwrap();
    assert_eq!(session["principal"]["user_id"], alice.user_id);
    assert_eq!(
        b.get(format!("{endpoint}/session"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let second_session = body(
        b.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(second_session["principal"]["user_id"], alice.user_id);
    store
        .lock()
        .unwrap()
        .db
        .execute("UPDATE identity_sessions SET expires_at_ms=0", [])
        .unwrap();
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let fresh = body(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let fresh_token = fresh["token"].as_str().unwrap();
    body(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(fresh_token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    body(
        a.post(format!("{endpoint}/logout"))
            .bearer_auth(fresh_token)
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(fresh_token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let before_rebind = body(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    {
        let work = store.lock().unwrap();
        identity::device::bind(&work.db, &admin, &first.id, None, "WinA", now + 1).unwrap();
    }
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(before_rebind["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let changed = body(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(changed["principal"]["user_id"], alice.user_id);
    {
        let mut network = network.lock().unwrap();
        let revision = network.revision();
        network
            .set_device(&revision, &first.id, "WinA".into(), BTreeSet::new(), true)
            .unwrap();
    }
    assert_eq!(
        a.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        a.get(format!("{endpoint}/session"))
            .bearer_auth(changed["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    body(
        b.post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    stop.send(()).unwrap();
    server.await.unwrap();
}
