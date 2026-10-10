use super::*;

#[test]
fn project_assignment_resumes_from_exact_approved_join_and_rejects_a_different_key() {
    let mut f = Fixture::new();
    let (_, csr) = csr();
    let request = f
        .owner
        .request_join("準備PC".into(), &csr, ip(), f.now)
        .unwrap();
    let snapshot = f.owner.snapshot(f.now);
    let key = snapshot["join_requests"][0]["key_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        f.owner
            .approved_join_device(&request.request_id, &key)
            .unwrap(),
        None
    );
    assert!(
        f.owner
            .approved_join_device(&request.request_id, &"0".repeat(64))
            .is_err()
    );
    f.owner
        .approve_join(
            &f.owner.revision(),
            &request.request_id,
            "準備PC".into(),
            BTreeSet::new(),
            f.now,
        )
        .unwrap();
    let id = f
        .owner
        .approved_join_device(&request.request_id, &key)
        .unwrap()
        .unwrap();
    assert_eq!(
        f.owner
            .approved_join_device(&request.request_id, &key)
            .unwrap(),
        Some(id.clone())
    );
    f.owner
        .set_admission(&f.owner.revision(), &id, true)
        .unwrap();
    assert!(
        f.owner
            .approved_join_device(&request.request_id, &key)
            .is_err()
    );
}

fn csr_at(key: &str, address: IpAddr, common_name: Option<&str>) -> String {
    let key = rcgen::KeyPair::from_pem(key).unwrap();
    let mut params = rcgen::CertificateParams::new(vec![address.to_string()]).unwrap();
    if let Some(name) = common_name {
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
    }
    params.serialize_request(&key).unwrap().pem().unwrap()
}
fn proof(key: &str, receipt: &network::JoinReceipt) -> String {
    csr_at(
        key,
        ip(),
        Some(&format!(
            "moyAI join {} {} {}",
            receipt.hub_id, receipt.request_id, receipt.challenge
        )),
    )
}

#[test]
fn join_requires_key_proof_and_exact_approval_and_survives_response_loss_and_restart() {
    let mut f = Fixture::new();
    let (key, csr) = csr();
    let first = f
        .owner
        .request_join("同名のPC".into(), &csr, ip(), f.now)
        .unwrap();
    let duplicate = f
        .owner
        .request_join(
            "client rename cannot rewrite approved label".into(),
            &csr,
            ip(),
            f.now,
        )
        .unwrap();
    assert_eq!(first.request_id, duplicate.request_id);
    assert_eq!(first.challenge, duplicate.challenge);
    let (_, other_csr) = self::csr();
    let other = f
        .owner
        .request_join("別のPC".into(), &other_csr, ip(), f.now)
        .unwrap();
    assert_ne!(first.request_id, other.request_id);
    let signed = proof(&key, &first);
    let wrong_key = proof(&rcgen::KeyPair::generate().unwrap().serialize_pem(), &first);
    assert!(
        f.owner
            .join_status(&first.request_id, &wrong_key, ip(), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .join_status(&other.request_id, &signed, ip(), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .join_status(&first.request_id, &csr, ip(), f.now)
            .is_err()
    );
    let before = std::fs::read(f.directory.path().join("network.json")).unwrap();
    assert_eq!(
        f.owner
            .join_status(&first.request_id, &signed, ip(), f.now)
            .unwrap()["status"],
        "pending"
    );
    assert_eq!(
        std::fs::read(f.directory.path().join("network.json")).unwrap(),
        before
    );
    assert!(
        f.owner.snapshot(f.now)["devices"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        f.owner
            .approve_join(
                &f.owner.revision(),
                &first.request_id,
                "A".into(),
                BTreeSet::from(["self-elected-admin".into()]),
                f.now
            )
            .is_err()
    );
    f.owner
        .approve_join(
            &f.owner.revision(),
            &first.request_id,
            "管理者が確認したPC".into(),
            BTreeSet::new(),
            f.now,
        )
        .unwrap();
    let issued = f
        .owner
        .join_status(&first.request_id, &signed, ip(), f.now)
        .unwrap();
    assert_eq!(issued["status"], "approved");
    assert_eq!(issued["certificate"]["label"], "管理者が確認したPC");
    let reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    assert_eq!(
        reopened
            .join_status(&first.request_id, &signed, ip(), f.now)
            .unwrap(),
        issued
    );
    assert!(
        reopened.current_certificate(
            issued["certificate"]["device_id"].as_str().unwrap(),
            issued["certificate"]["certificate_sha256"]
                .as_str()
                .unwrap(),
            f.now
        )
    );
    assert_eq!(
        reopened
            .join_status(&other.request_id, &proof(&key, &other), ip(), f.now)
            .unwrap_err()
            .to_string(),
        "unauthorized"
    );
    let repeated = f
        .owner
        .request_join("ignored".into(), &csr, ip(), f.now)
        .unwrap();
    assert_eq!(repeated.request_id, first.request_id);
}

#[test]
fn join_ip_change_replaces_pending_approval_target_and_does_not_widen_the_signed_san() {
    let mut f = Fixture::new();
    let (key, csr) = csr();
    let old = f.owner.request_join("A".into(), &csr, ip(), f.now).unwrap();
    let old_revision = f.owner.revision();
    let changed_ip = IpAddr::from([127, 0, 0, 2]);
    assert!(
        f.owner
            .request_join("A".into(), &csr, changed_ip, f.now)
            .is_err()
    );
    let changed_csr = csr_at(&key, changed_ip, None);
    let new = f
        .owner
        .request_join("A".into(), &changed_csr, changed_ip, f.now + 1)
        .unwrap();
    assert_ne!(old.request_id, new.request_id);
    assert!(
        f.owner
            .approve_join(
                &old_revision,
                &old.request_id,
                "A".into(),
                BTreeSet::new(),
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .approve_join(
                &f.owner.revision(),
                &old.request_id,
                "A".into(),
                BTreeSet::new(),
                f.now
            )
            .is_err()
    );
    let row = &f.owner.snapshot(f.now)["join_requests"][0];
    assert_eq!(row["observed_ip"], "127.0.0.2");
    f.owner
        .approve_join(
            &f.owner.revision(),
            &new.request_id,
            "A".into(),
            BTreeSet::new(),
            f.now,
        )
        .unwrap();
    let signed = csr_at(
        &key,
        changed_ip,
        Some(&format!(
            "moyAI join {} {} {}",
            new.hub_id, new.request_id, new.challenge
        )),
    );
    let issued = f
        .owner
        .join_status(&new.request_id, &signed, changed_ip, f.now)
        .unwrap();
    assert!(
        network::tls::verify_server_chain(
            issued["certificate"]["certificate_pem"].as_str().unwrap(),
            f.owner.ca_certificate(),
            changed_ip
        )
        .is_ok()
    );
    assert!(
        network::tls::verify_server_chain(
            issued["certificate"]["certificate_pem"].as_str().unwrap(),
            f.owner.ca_certificate(),
            ip()
        )
        .is_err()
    );
    assert!(
        f.owner
            .join_status(&new.request_id, &signed, ip(), f.now)
            .is_err()
    );
}

#[test]
fn expired_and_flooded_join_requests_do_not_consume_registered_device_capacity() {
    let mut f = Fixture::new();
    let registered = f.device("registered");
    let (key, csr) = csr();
    let old = f
        .owner
        .request_join("waiting".into(), &csr, ip(), f.now)
        .unwrap();
    for n in 0..7 {
        f.owner
            .request_join(format!("pending {n}"), &self::csr().1, ip(), f.now)
            .unwrap();
    }
    assert!(matches!(
        f.owner
            .request_join("flood".into(), &self::csr().1, ip(), f.now),
        Err(NetworkError::Full)
    ));
    f.owner
        .seen(&registered.receipt.device_id, ip(), f.now)
        .unwrap();
    assert!(f.owner.current_certificate(
        &registered.receipt.device_id,
        &registered.receipt.certificate_sha256,
        f.now
    ));
    assert_eq!(
        f.owner.snapshot(f.now)["devices"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        f.owner
            .join_status(&old.request_id, &proof(&key, &old), ip(), old.expires_at_ms)
            .unwrap()["status"],
        "expired"
    );
    assert!(
        f.owner
            .approve_join(
                &f.owner.revision(),
                &old.request_id,
                "late".into(),
                BTreeSet::new(),
                old.expires_at_ms
            )
            .is_err()
    );
    let new = f
        .owner
        .request_join("fresh".into(), &csr, ip(), old.expires_at_ms)
        .unwrap();
    assert_ne!(new.request_id, old.request_id);
    assert_ne!(new.challenge, old.challenge);
    assert_eq!(
        f.owner.snapshot(old.expires_at_ms)["join_requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn stopped_devices_keep_exact_accepted_controls_but_old_execution_never_resumes() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    let accepted = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "accepted"), f.now)
        .unwrap();
    f.owner
        .accept_job(&b.receipt.device_id, &accepted.grant_id, "job", f.now)
        .unwrap();
    let unaccepted = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "unaccepted"), f.now)
        .unwrap();
    for device in [&a, &b] {
        f.owner
            .set_admission(&f.owner.revision(), &device.receipt.device_id, true)
            .unwrap();
    }
    for device in [&a, &b] {
        assert!(f.owner.current_certificate(
            &device.receipt.device_id,
            &device.receipt.certificate_sha256,
            f.now
        ));
        assert_eq!(
            f.owner
                .self_projection(&device.receipt.device_id, f.now)
                .unwrap()["admission"],
            "stopped"
        );
        assert!(
            f.owner
                .directory(&device.receipt.device_id, None, f.now)
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "new"), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .accept_job(
                &b.receipt.device_id,
                &unaccepted.grant_id,
                "too-late",
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .grant(
                &a.receipt.device_id,
                control_request(&b, "unaccepted", Action::Observe),
                f.now
            )
            .is_err()
    );
    for action in [Action::Observe, Action::Cancel] {
        let control = f
            .owner
            .grant(
                &a.receipt.device_id,
                control_request(&b, "accepted", action),
                f.now,
            )
            .unwrap();
        assert!(
            f.owner
                .introspect(
                    &b.receipt.device_id,
                    &control.token,
                    &a.receipt.certificate_sha256,
                    action,
                    f.now
                )
                .is_ok()
        );
        assert!(
            f.owner
                .introspect(
                    &b.receipt.device_id,
                    &control.token,
                    &a.receipt.certificate_sha256,
                    Action::Inspect,
                    f.now
                )
                .is_ok()
        );
        assert!(
            f.owner
                .introspect(
                    &b.receipt.device_id,
                    &control.token,
                    &a.receipt.certificate_sha256,
                    Action::Execute,
                    f.now
                )
                .is_err()
        );
    }
    f.owner
        .settle_job(
            &b.receipt.device_id,
            &accepted.grant_id,
            "job",
            network::Terminal::Cancelled,
            f.now,
        )
        .unwrap();
    for device in [&a, &b] {
        f.owner
            .set_admission(&f.owner.revision(), &device.receipt.device_id, false)
            .unwrap();
    }
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &unaccepted.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "unaccepted"), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "fresh-key"), f.now)
            .is_ok()
    );
    f.owner
        .set_device(
            &f.owner.revision(),
            &b.receipt.device_id,
            "B".into(),
            BTreeSet::new(),
            true,
        )
        .unwrap();
    assert!(
        f.owner
            .set_admission(&f.owner.revision(), &b.receipt.device_id, false)
            .is_err()
    );
    assert!(
        f.owner
            .authenticate(&b.receipt.certificate_sha256, f.now)
            .is_err()
    );
}

#[test]
fn stopped_model_permit_revision_does_not_revive_and_label_or_renewal_does_not_retire_it() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let revision = f
        .owner
        .admission_revision(&a.receipt.device_id, f.now)
        .unwrap();
    assert!(
        f.owner
            .authorize_model_permit(&a.receipt.certificate_sha256, revision, f.now)
            .is_ok()
    );
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "Renamed".into(),
            BTreeSet::new(),
            false,
        )
        .unwrap();
    assert_eq!(
        f.owner
            .admission_revision(&a.receipt.device_id, f.now)
            .unwrap(),
        revision
    );
    let renewed = f
        .owner
        .renew(&a.receipt.device_id, &a.csr, ip(), f.now)
        .unwrap();
    assert!(
        f.owner
            .authorize_model_permit(&renewed.certificate_sha256, revision, f.now)
            .is_ok()
    );
    f.owner
        .set_admission(&f.owner.revision(), &a.receipt.device_id, true)
        .unwrap();
    assert!(
        f.owner
            .authorize_model_permit(&renewed.certificate_sha256, revision, f.now)
            .is_err()
    );
    let re_renewed = f
        .owner
        .renew(&a.receipt.device_id, &a.csr, ip(), f.now)
        .unwrap();
    assert_eq!(re_renewed.device_id, a.receipt.device_id);
    f.owner
        .set_admission(&f.owner.revision(), &a.receipt.device_id, false)
        .unwrap();
    let current = f
        .owner
        .admission_revision(&a.receipt.device_id, f.now)
        .unwrap();
    assert_ne!(current, revision);
    assert!(
        f.owner
            .authorize_model_permit(&re_renewed.certificate_sha256, revision, f.now)
            .is_err()
    );
    assert!(
        f.owner
            .authorize_model_permit(&re_renewed.certificate_sha256, current, f.now)
            .is_ok()
    );
}

#[test]
fn schema_two_preserves_accepted_authority_and_strict_join_fields_on_forward_load() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    let grant = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "accepted"), f.now)
        .unwrap();
    f.owner
        .accept_job(&b.receipt.device_id, &grant.grant_id, "canonical", f.now)
        .unwrap();
    let path = f.directory.path().join("network.json");
    let mut old: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["schema_version"] = json!(2);
    old.as_object_mut().unwrap().remove("join_requests");
    for device in old["devices"].as_object_mut().unwrap().values_mut() {
        for field in [
            "stopped",
            "admission_revision",
            "join_approval",
            "deleted_at_ms",
        ] {
            device.as_object_mut().unwrap().remove(field);
        }
    }
    let old = serde_json::to_vec(&old).unwrap();
    std::fs::write(&path, &old).unwrap();
    f.owner = DeviceNetwork::open(&path, "hub-test").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), old);
    f.publish(&b, true);
    let recovered = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "accepted"), f.now)
        .unwrap();
    assert_eq!(recovered.grant_id, grant.grant_id);
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "name only".into(),
            BTreeSet::new(),
            false,
        )
        .unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &recovered.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_ok()
    );
    let pending = f
        .owner
        .request_join("pending".into(), &csr().1, ip(), f.now)
        .unwrap();
    let mut invalid: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(invalid["schema_version"], 5);
    invalid["join_requests"][&pending.request_id]["admin"] = true.into();
    let bytes = serde_json::to_vec(&invalid).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(DeviceNetwork::open(&path, "hub-test").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn actual_tls_join_exposes_only_proof_endpoints_until_approval_and_stopped_prepare_is_denied()
{
    let f = Fixture::new();
    let (server_cert, server_key) = f.owner.server_certificate(ip(), f.now).unwrap();
    let ca = f.owner.ca_certificate().to_owned();
    let anonymous = http(&ca, None);
    let network = Arc::new(Mutex::new(f.owner));
    let core = Arc::new(Mutex::new(
        HubCore::new(CatalogStore::empty("test")).unwrap(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let listener = network::tls::NetworkListener::new(
        listener,
        network::tls::acceptor(&server_cert, &server_key, &ca, true).unwrap(),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = service::network_router(core, network.clone(), None);
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<network::tls::TlsPeer>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let (key, csr) = csr();
    let reply = anonymous
        .post(format!("{endpoint}/v1/network/join/request"))
        .json(&json!({"label":"申請PC","csr_pem":csr}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    let receipt: Value = reply.json().await.unwrap();
    let request = network::JoinReceipt {
        hub_id: receipt["hub_id"].as_str().unwrap().into(),
        request_id: receipt["request_id"].as_str().unwrap().into(),
        challenge: receipt["challenge"].as_str().unwrap().into(),
        expires_at_ms: receipt["expires_at_ms"].as_u64().unwrap(),
    };
    let signed = proof(&key, &request);
    let status_body = json!({"request_id":request.request_id,"proof_csr_pem":signed});
    for path in [
        "/v1/network/self",
        "/v1/catalog",
        "/v1/network/join/request",
    ] {
        assert_eq!(
            anonymous
                .get(format!("{endpoint}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    for path in [
        "/v1/network/model-session",
        "/v1/network/grants",
        "/v1/requests/prepare",
    ] {
        assert_eq!(
            anonymous
                .post(format!("{endpoint}{path}"))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    assert_eq!(
        anonymous
            .post(format!("{endpoint}/v1/network/join/status"))
            .header("Origin", "https://example.invalid")
            .json(&status_body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(anonymous.post(format!("{endpoint}/v1/network/join/status")).json(&json!({"request_id":request.request_id,"proof_csr_pem":proof(&rcgen::KeyPair::generate().unwrap().serialize_pem(), &request)})).send().await.unwrap().status(), 401);
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner
            .approve_join(
                &revision,
                &request.request_id,
                "承認PC".into(),
                BTreeSet::new(),
                f.now,
            )
            .unwrap();
    }
    let issued: Value = anonymous
        .post(format!("{endpoint}/v1/network/join/status"))
        .json(&status_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cert = &issued["certificate"];
    let device = Device {
        receipt: CertificateReceipt {
            hub_id: cert["hub_id"].as_str().unwrap().into(),
            device_id: cert["device_id"].as_str().unwrap().into(),
            label: cert["label"].as_str().unwrap().into(),
            certificate_pem: cert["certificate_pem"].as_str().unwrap().into(),
            ca_certificate_pem: cert["ca_certificate_pem"].as_str().unwrap().into(),
            certificate_sha256: cert["certificate_sha256"].as_str().unwrap().into(),
            expires_at_ms: cert["expires_at_ms"].as_u64().unwrap(),
        },
        key,
        csr,
    };
    let client = http(&ca, Some(&device));
    let session: Value = client
        .post(format!("{endpoint}/v1/network/model-session"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner
            .set_admission(&revision, &device.receipt.device_id, true)
            .unwrap();
    }
    let own: Value = client
        .get(format!("{endpoint}/v1/network/self"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(own["admission"], "stopped");
    let blocked = client.post(format!("{endpoint}/v1/requests/prepare")).bearer_auth(session["client_token"].as_str().unwrap()).json(&json!({"id":session["id"],"context":"main","turn_id":"turn","request_id":"request","expected_hub_id":"hub-test","reviewed_revision":"1"})).send().await.unwrap();
    assert_eq!(blocked.status(), 403);
    assert_eq!(
        blocked.json::<Value>().await.unwrap()["error"],
        "device_stopped"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[test]
fn same_windows_username_remains_metadata_on_distinct_registered_keys() {
    let mut fixture = Fixture::new();
    let first = fixture
        .owner
        .request_join_with_user(
            "WinA".into(),
            Some("DOMAIN\\same-user".into()),
            &csr().1,
            ip(),
            fixture.now,
        )
        .unwrap();
    let second = fixture
        .owner
        .request_join_with_user(
            "WinB".into(),
            Some("DOMAIN\\same-user".into()),
            &csr().1,
            ip(),
            fixture.now,
        )
        .unwrap();
    let pending = fixture.owner.snapshot(fixture.now);
    let requests = pending["join_requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0]["key_sha256"], requests[1]["key_sha256"]);
    for (request, label) in [(&first, "WinA"), (&second, "WinB")] {
        fixture
            .owner
            .approve_join(
                &fixture.owner.revision(),
                &request.request_id,
                label.into(),
                Default::default(),
                fixture.now,
            )
            .unwrap();
    }
    let snapshot = fixture.owner.snapshot(fixture.now);
    let rows = snapshot["devices"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["device_id"], rows[1]["device_id"]);
    assert!(
        rows.iter()
            .all(|row| row["windows_username"] == "DOMAIN\\same-user")
    );
    for row in rows {
        let own = fixture
            .owner
            .self_projection(row["device_id"].as_str().unwrap(), fixture.now)
            .unwrap();
        // Account labels stay in administrator views. Existing Desktop clients
        // deserialize this device-authentication response with strict fields.
        assert_eq!(
            own,
            json!({
                "hub_id": "hub-test", "device_id": row["device_id"],
                "label": row["label"], "groups": row["groups"],
                "revision": snapshot["revision"], "admission": "allowed",
                "certificate_sha256": row["certificate_sha256"],
                "expires_at_ms": row["expires_at_ms"], "cancelled_lineages": [],
            })
        );
    }
    let path = fixture.directory.path().join("network.json");
    let reopened = DeviceNetwork::open(&path, "hub-test").unwrap();
    assert_eq!(
        reopened.snapshot(fixture.now)["devices"],
        snapshot["devices"]
    );
}

#[test]
fn schema_four_load_preserves_missing_windows_usernames_until_forward_save() {
    let mut fixture = Fixture::new();
    let device = fixture.device("Existing PC");
    let pending = fixture
        .owner
        .request_join_with_user(
            "Pending PC".into(),
            Some("DOMAIN\\new-user".into()),
            &csr().1,
            ip(),
            fixture.now,
        )
        .unwrap();
    let path = fixture.directory.path().join("network.json");
    let mut old: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["schema_version"] = json!(4);
    old["devices"][&device.receipt.device_id]
        .as_object_mut()
        .unwrap()
        .remove("windows_username");
    old["join_requests"][&pending.request_id]
        .as_object_mut()
        .unwrap()
        .remove("windows_username");
    let bytes = serde_json::to_vec(&old).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let mut reopened = DeviceNetwork::open(&path, "hub-test").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(reopened.current_certificate(
        &device.receipt.device_id,
        &device.receipt.certificate_sha256,
        fixture.now
    ));
    let snapshot = reopened.snapshot(fixture.now);
    assert!(snapshot["devices"][0]["windows_username"].is_null());
    assert!(snapshot["join_requests"][0]["windows_username"].is_null());
    reopened
        .set_gateway_port(&reopened.revision(), 9550)
        .unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["schema_version"], 5);
    assert!(saved["devices"][&device.receipt.device_id]["windows_username"].is_null());
    assert!(saved["join_requests"][&pending.request_id]["windows_username"].is_null());
}
