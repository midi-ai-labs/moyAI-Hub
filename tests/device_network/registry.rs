use super::*;

#[test]
fn names_are_unique_across_pending_registered_stopped_and_revoked_devices() {
    let mut f = Fixture::new();
    let a = f.device("  WinA  ");
    assert_eq!(a.receipt.label, "WinA");
    for name in ["wina", " WINa\u{3000}"] {
        assert!(matches!(
            f.owner.request_join(name.into(), &csr().1, ip(), f.now),
            Err(NetworkError::DeviceNameConflict)
        ));
    }
    let b = f.device("WinB");
    let revision = f.owner.revision();
    assert!(matches!(
        f.owner.set_device(
            &revision,
            &b.receipt.device_id,
            "WINA".into(),
            BTreeSet::new(),
            false
        ),
        Err(NetworkError::DeviceNameConflict)
    ));
    assert_eq!(f.owner.revision(), revision);
    f.owner
        .set_admission(&revision, &a.receipt.device_id, true)
        .unwrap();
    assert!(matches!(
        f.owner
            .invite(&f.owner.revision(), "wina".into(), BTreeSet::new(), f.now),
        Err(NetworkError::DeviceNameConflict)
    ));
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "WinA".into(),
            BTreeSet::new(),
            true,
        )
        .unwrap();
    assert!(matches!(
        f.owner.request_join("wina".into(), &csr().1, ip(), f.now),
        Err(NetworkError::DeviceNameConflict)
    ));
    let pending = f
        .owner
        .request_join("Pending".into(), &csr().1, ip(), f.now)
        .unwrap();
    assert!(matches!(
        f.owner
            .request_join(" PENDING ".into(), &csr().1, ip(), f.now),
        Err(NetworkError::DeviceNameConflict)
    ));
    assert!(matches!(
        f.owner.approve_join(
            &f.owner.revision(),
            &pending.request_id,
            "winb".into(),
            BTreeSet::new(),
            f.now
        ),
        Err(NetworkError::DeviceNameConflict)
    ));
    f.owner
        .approve_join(
            &f.owner.revision(),
            &pending.request_id,
            "pending".into(),
            BTreeSet::new(),
            f.now,
        )
        .unwrap();
    assert!(
        f.owner
            .request_join("\u{3000}".into(), &csr().1, ip(), f.now)
            .is_err()
    );
    let accented = f.device("Étage");
    assert_eq!(accented.receipt.label, "Étage");
    assert!(matches!(
        f.owner.request_join("étage".into(), &csr().1, ip(), f.now),
        Err(NetworkError::DeviceNameConflict)
    ));
    // No implicit canonical/compatibility conversion or locale-specific case folding.
    f.device("E\u{301}tage");
    f.device("ＷｉｎＡ");
}

#[test]
fn simultaneous_name_requests_share_the_save_boundary_and_keep_one_identity() {
    let f = Fixture::new();
    let owner = Arc::new(Mutex::new(f.owner));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = ["Twin", " TWIN "]
        .into_iter()
        .map(|label| {
            let owner = owner.clone();
            let barrier = barrier.clone();
            let now = f.now;
            let csr = csr().1;
            std::thread::spawn(move || {
                barrier.wait();
                owner
                    .lock()
                    .unwrap()
                    .request_join(label.into(), &csr, ip(), now)
                    .map(|r| r.request_id)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(NetworkError::DeviceNameConflict)))
            .count(),
        1
    );
    assert_eq!(
        owner.lock().unwrap().snapshot(f.now)["join_requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn legacy_duplicates_remain_addressable_and_can_be_resolved_without_merging() {
    let mut f = Fixture::new();
    let a = f.device("WinB");
    let b = f.device("WinB-02");
    let path = f.directory.path().join("network.json");
    let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy["schema_version"] = json!(3);
    for device in legacy["devices"].as_object_mut().unwrap().values_mut() {
        device.as_object_mut().unwrap().remove("deleted_at_ms");
    }
    legacy["devices"][&b.receipt.device_id]["label"] = json!(" winb ");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    f.owner = DeviceNetwork::open(&path, "hub-test").unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "load does not silently rewrite a released store"
    );
    assert!(
        f.owner.snapshot(f.now)["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["name_conflict"] == true)
    );
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "WinB".into(),
            BTreeSet::new(),
            false,
        )
        .unwrap();
    f.owner
        .set_device(
            &f.owner.revision(),
            &b.receipt.device_id,
            "WinB-02".into(),
            BTreeSet::new(),
            false,
        )
        .unwrap();
    assert!(
        f.owner.snapshot(f.now)["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["name_conflict"] == false)
    );
    assert!(f.owner.current_certificate(
        &a.receipt.device_id,
        &a.receipt.certificate_sha256,
        f.now
    ));
    assert!(f.owner.current_certificate(
        &b.receipt.device_id,
        &b.receipt.certificate_sha256,
        f.now
    ));
}

#[test]
fn deleting_old_registration_preserves_other_ids_rules_and_history_but_retires_credentials() {
    use network::history::{
        HistoryDirection, HistoryPage, HistoryQuery, HistoryResponse, HistorySubmission,
    };
    let mut f = Fixture::new();
    let a = f.device("WinA");
    let old = f.device("WinB");
    let b = f.device("WinB-02");
    f.publish(&old, true);
    f.publish(&b, true);
    f.allow(&a, &old, Usage::Direct);
    let kept_rule = f.allow(&a, &b, Usage::Direct);
    let grant = f
        .owner
        .grant(&a.receipt.device_id, request(&old, "unaccepted"), f.now)
        .unwrap();
    let history = f
        .owner
        .request_history(
            &old.receipt.device_id,
            HistoryQuery {
                direction: HistoryDirection::Execution,
                offset: 0,
                anchor: None,
                id: None,
            },
            f.now,
        )
        .unwrap();
    f.owner
        .submit_history(
            &old.receipt.device_id,
            HistorySubmission {
                request_id: history.request_id.clone(),
                response: Some(HistoryResponse::Page(HistoryPage {
                    rows: vec![],
                    anchor: None,
                    next_offset: None,
                })),
                error: None,
            },
            f.now,
        )
        .unwrap();
    let stale = f.owner.revision();
    f.owner
        .set_admission(&stale, &old.receipt.device_id, true)
        .unwrap();
    assert!(matches!(
        f.owner.delete_device(&stale, &old.receipt.device_id, f.now),
        Err(NetworkError::StaleRevision)
    ));
    f.owner
        .delete_device(&f.owner.revision(), &old.receipt.device_id, f.now)
        .unwrap();
    let snapshot = f.owner.snapshot(f.now);
    assert_eq!(snapshot["devices"].as_array().unwrap().len(), 2);
    assert_eq!(
        snapshot["deleted_devices"][0]["device_id"],
        old.receipt.device_id
    );
    assert_eq!(snapshot["rules"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["rules"][0]["id"], kept_rule);
    assert!(!f.owner.current_certificate(
        &old.receipt.device_id,
        &old.receipt.certificate_sha256,
        f.now
    ));
    assert!(f.owner.current_certificate(
        &b.receipt.device_id,
        &b.receipt.certificate_sha256,
        f.now
    ));
    assert!(
        f.owner
            .introspect(
                &old.receipt.device_id,
                &grant.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .renew(&old.receipt.device_id, &old.csr, ip(), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .request_join("WinB".into(), &old.csr, ip(), f.now)
            .is_err()
    );
    let cached = f.owner.history_result(&history.request_id, f.now).unwrap();
    assert_eq!(cached.device_id, old.receipt.device_id);
    assert!(cached.cached);
    assert_eq!(cached.device_label, "WinB");
    let reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    f.owner = reopened;
    assert!(!f.owner.current_certificate(
        &old.receipt.device_id,
        &old.receipt.certificate_sha256,
        f.now
    ));
    assert!(
        f.owner
            .set_admission(&f.owner.revision(), &old.receipt.device_id, false)
            .is_err()
    );
    let invitation = f
        .owner
        .invite(&f.owner.revision(), "WinB".into(), BTreeSet::new(), f.now)
        .unwrap();
    assert!(
        f.owner
            .enroll(invitation["code"].as_str().unwrap(), &old.csr, ip(), f.now)
            .is_err()
    );
    let replacement = f
        .owner
        .enroll(invitation["code"].as_str().unwrap(), &csr().1, ip(), f.now)
        .unwrap();
    assert_ne!(replacement.device_id, old.receipt.device_id);
    assert_eq!(replacement.label, "WinB");
    assert_eq!(
        f.owner.snapshot(f.now)["rules"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn deletion_refuses_unsettled_work_even_after_restart_until_the_receiver_confirms_terminal() {
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
        .accept_job(&b.receipt.device_id, &grant.grant_id, "job", f.now)
        .unwrap();
    f.owner = DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    for device in [&a, &b] {
        let before = std::fs::read(f.directory.path().join("network.json")).unwrap();
        assert_eq!(f.owner.unsettled_device_jobs(&device.receipt.device_id), 1);
        assert!(matches!(
            f.owner
                .delete_device(&f.owner.revision(), &device.receipt.device_id, f.now),
            Err(NetworkError::DeviceWorkUnsettled)
        ));
        assert_eq!(
            std::fs::read(f.directory.path().join("network.json")).unwrap(),
            before
        );
    }
    f.owner
        .settle_job(
            &b.receipt.device_id,
            &grant.grant_id,
            "job",
            network::Terminal::Completed,
            f.now,
        )
        .unwrap();
    f.owner
        .delete_device(&f.owner.revision(), &b.receipt.device_id, f.now)
        .unwrap();
    assert_eq!(f.owner.unsettled_device_jobs(&b.receipt.device_id), 0);
}

#[test]
fn deletion_rejects_subsequent_model_permit_claims() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let revision = f
        .owner
        .admission_revision(&a.receipt.device_id, f.now)
        .unwrap();
    f.owner
        .authorize_model_permit(&a.receipt.certificate_sha256, revision, f.now)
        .unwrap();
    f.owner
        .delete_device(&f.owner.revision(), &a.receipt.device_id, f.now)
        .unwrap();
    assert!(
        f.owner
            .authorize_model_permit(&a.receipt.certificate_sha256, revision, f.now)
            .is_err()
    );
    // This contract only denies a subsequent claim. Existing gateway streams
    // have their own terminal owner and are never declared completed here.
}
