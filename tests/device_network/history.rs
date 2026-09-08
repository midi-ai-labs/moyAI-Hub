use super::*;
use network::history::{
    HistoryDetail, HistoryDirection, HistoryPage, HistoryQuery, HistoryResponse, HistoryRow,
    HistoryStatus, HistorySubmission,
};

fn query(direction: HistoryDirection) -> HistoryQuery {
    HistoryQuery {
        direction,
        offset: 0,
        anchor: None,
        id: None,
    }
}
fn row(direction: HistoryDirection) -> HistoryRow {
    HistoryRow {
        id: ulid::Ulid::new().to_string(),
        direction,
        created_at_ms: 10,
        updated_at_ms: None,
        title: "履歴の照会 <script>".into(),
        peer_label: "WinB".into(),
        target_label: "temp".into(),
        state: "completed".into(),
        stop_status: "confirmed".into(),
        state_source: "last_observed".into(),
        result_received: false,
        session_id: ulid::Ulid::new().to_string(),
        profile_id: "profile".into(),
        job_id: Some("worker-job".into()),
        root_task_id: "root".into(),
        device_path: vec!["WinA".into(), "WinB".into()],
        can_stop: false,
    }
}
fn empty(request_id: &str) -> HistorySubmission {
    HistorySubmission {
        request_id: request_id.into(),
        response: Some(HistoryResponse::Page(HistoryPage {
            rows: vec![],
            anchor: None,
            next_offset: None,
        })),
        error: None,
    }
}
fn ready(f: &mut Fixture, device: &Device) {
    f.owner
        .seen(&device.receipt.device_id, ip(), f.now)
        .unwrap();
}

#[test]
fn history_requests_are_exact_device_read_only_and_allow_stopped_receivers() {
    let mut f = Fixture::new();
    let a = f.device("WinA");
    let b = f.device("WinB");
    ready(&mut f, &a);
    ready(&mut f, &b);
    f.owner
        .set_admission(&f.owner.revision(), &a.receipt.device_id, true)
        .unwrap();
    let request = f
        .owner
        .request_history(
            &a.receipt.device_id,
            query(HistoryDirection::Instruction),
            f.now,
        )
        .unwrap();
    assert_eq!(request.device_admission, "stopped");
    assert!(request.device_online);
    assert_eq!(request.status, HistoryStatus::Pending);
    let queued = f
        .owner
        .history_requests(&a.receipt.device_id, f.now)
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].device_id, a.receipt.device_id);
    assert_eq!(queued[0].expires_at_ms, f.now + 60_000);
    assert!(
        f.owner
            .history_requests(&b.receipt.device_id, f.now)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        f.owner
            .submit_history(&b.receipt.device_id, empty(&request.request_id), f.now),
        Err(NetworkError::Unauthorized)
    ));
    let repeated = f
        .owner
        .request_history(
            &a.receipt.device_id,
            query(HistoryDirection::Instruction),
            f.now,
        )
        .unwrap();
    assert_eq!(repeated.request_id, request.request_id);
    assert!(matches!(
        f.owner.request_history(
            &a.receipt.device_id,
            query(HistoryDirection::Execution),
            f.now
        ),
        Err(NetworkError::Full)
    ));
    f.owner
        .submit_history(&a.receipt.device_id, empty(&request.request_id), f.now + 1)
        .unwrap();
    f.owner
        .submit_history(&a.receipt.device_id, empty(&request.request_id), f.now + 2)
        .unwrap();
    let result = f
        .owner
        .history_result(&request.request_id, f.now + 31_000)
        .unwrap();
    assert_eq!(result.status, HistoryStatus::Completed);
    assert!(result.cached);
    assert!(!result.device_online);
    assert_eq!(result.completed_at_ms, Some(f.now + 1));
    assert!(
        f.owner
            .history_requests(&a.receipt.device_id, f.now + 31_000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn history_validates_direction_cursor_job_response_and_bounds_before_settlement() {
    let mut f = Fixture::new();
    let a = f.device("WinA");
    ready(&mut f, &a);
    for anchor in [
        "execution:1",
        "instruction:0",
        "instruction:-1",
        "instruction:9223372036854775808",
        "123:01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ] {
        let mut q = query(HistoryDirection::Instruction);
        q.offset = 1;
        q.anchor = Some(anchor.into());
        assert!(
            f.owner
                .request_history(&a.receipt.device_id, q, f.now)
                .is_err()
        );
    }
    let mut q = query(HistoryDirection::Instruction);
    q.offset = 2;
    q.anchor = Some("instruction:50".into());
    let request = f
        .owner
        .request_history(&a.receipt.device_id, q, f.now)
        .unwrap();
    let good_row = row(HistoryDirection::Instruction);
    let good = HistorySubmission {
        request_id: request.request_id.clone(),
        response: Some(HistoryResponse::Page(HistoryPage {
            rows: vec![good_row.clone()],
            next_offset: Some(3),
            anchor: Some("instruction:50".into()),
        })),
        error: None,
    };
    let mut invalid = good.clone();
    if let Some(HistoryResponse::Page(page)) = &mut invalid.response {
        page.anchor = Some("execution:50".into());
    }
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, invalid, f.now)
            .is_err()
    );
    let mut invalid = good.clone();
    if let Some(HistoryResponse::Page(page)) = &mut invalid.response {
        page.rows[0].direction = HistoryDirection::Execution;
    }
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, invalid, f.now)
            .is_err()
    );
    let mut invalid = good.clone();
    if let Some(HistoryResponse::Page(page)) = &mut invalid.response {
        page.next_offset = Some(4);
    }
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, invalid, f.now)
            .is_err()
    );
    f.owner
        .submit_history(&a.receipt.device_id, good, f.now)
        .unwrap();
    let mut q = query(HistoryDirection::Execution);
    let detail_row = row(HistoryDirection::Execution);
    q.id = Some(detail_row.id.clone());
    let detail_request = f
        .owner
        .request_history(&a.receipt.device_id, q, f.now)
        .unwrap();
    let valid = HistorySubmission {
        request_id: detail_request.request_id.clone(),
        response: Some(HistoryResponse::Detail(HistoryDetail {
            row: detail_row,
            markdown: "履歴\n".into(),
            truncated: false,
        })),
        error: None,
    };
    let mut wrong_id = valid.clone();
    if let Some(HistoryResponse::Detail(detail)) = &mut wrong_id.response {
        detail.row.id = good_row.id;
    }
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, wrong_id, f.now)
            .is_err()
    );
    let mut oversized = valid.clone();
    if let Some(HistoryResponse::Detail(detail)) = &mut oversized.response {
        detail.markdown = "x".repeat(64 * 1024 + 1);
    }
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, oversized, f.now)
            .is_err()
    );
    let mut ambiguous = valid.clone();
    ambiguous.error = Some("history_unavailable".into());
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, ambiguous, f.now)
            .is_err()
    );
    assert_eq!(
        f.owner
            .history_result(&detail_request.request_id, f.now)
            .unwrap()
            .status,
        HistoryStatus::Pending
    );
    f.owner
        .submit_history(&a.receipt.device_id, valid, f.now)
        .unwrap();
}

#[test]
fn history_expiry_revocation_and_bounded_cache_do_not_persist_task_content() {
    let mut f = Fixture::new();
    let a = f.device("WinA");
    ready(&mut f, &a);
    let bytes = std::fs::read(f.directory.path().join("network.json")).unwrap();
    let expired = f
        .owner
        .request_history(
            &a.receipt.device_id,
            query(HistoryDirection::Instruction),
            f.now,
        )
        .unwrap();
    assert_eq!(
        f.owner
            .history_result(&expired.request_id, f.now + 60_000)
            .unwrap()
            .status,
        HistoryStatus::Expired
    );
    assert!(matches!(
        f.owner.submit_history(
            &a.receipt.device_id,
            empty(&expired.request_id),
            f.now + 60_000
        ),
        Err(NetworkError::StaleRevision)
    ));
    f.now += 60_001;
    ready(&mut f, &a);
    let mut ids = Vec::new();
    for i in 0..66 {
        let request = f
            .owner
            .request_history(
                &a.receipt.device_id,
                query(HistoryDirection::Instruction),
                f.now + i,
            )
            .unwrap();
        f.owner
            .submit_history(&a.receipt.device_id, empty(&request.request_id), f.now + i)
            .unwrap();
        ids.push(request.request_id);
    }
    assert!(f.owner.history_result(&ids[0], f.now + 100).is_err());
    assert!(f.owner.history_result(&ids[1], f.now + 100).is_err());
    assert!(
        ids[2..]
            .iter()
            .all(|id| f.owner.history_result(id, f.now + 100).is_ok())
    );
    assert_eq!(
        std::fs::read(f.directory.path().join("network.json")).unwrap(),
        bytes
    );
    let mut reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    assert!(
        reopened
            .history_result(ids.last().unwrap(), f.now + 100)
            .is_err()
    );
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "WinA".into(),
            BTreeSet::new(),
            true,
        )
        .unwrap();
    assert!(
        f.owner
            .history_requests(&a.receipt.device_id, f.now)
            .is_err()
    );
    assert!(
        f.owner
            .request_history(
                &a.receipt.device_id,
                query(HistoryDirection::Execution),
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .submit_history(&a.receipt.device_id, empty(ids.last().unwrap()), f.now)
            .is_err()
    );
    let cached = f
        .owner
        .history_result(ids.last().unwrap(), f.now + 100)
        .unwrap();
    assert!(!cached.device_online);
    assert_eq!(cached.device_admission, "revoked");
    assert!(
        f.owner
            .history_result(ids.last().unwrap(), f.now + 30 * 60_000 + 100)
            .is_err()
    );
}

#[tokio::test]
async fn history_http_tls_is_device_scoped_has_no_remote_admin_and_limits_large_results() {
    let mut f = Fixture::new();
    let a = f.device("WinA");
    let b = f.device("WinB");
    ready(&mut f, &a);
    let detail_row = row(HistoryDirection::Execution);
    let mut q = query(HistoryDirection::Execution);
    q.id = Some(detail_row.id.clone());
    let request = f
        .owner
        .request_history(&a.receipt.device_id, q, f.now)
        .unwrap();
    let ca = f.owner.ca_certificate().to_string();
    let (cert, key) = f.owner.server_certificate(ip(), f.now).unwrap();
    let network = Arc::new(Mutex::new(f.owner));
    let core = Arc::new(Mutex::new(
        HubCore::new(CatalogStore::empty("test")).unwrap(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let listener = network::tls::NetworkListener::new(
        listener,
        network::tls::acceptor(&cert, &key, &ca, true).unwrap(),
    );
    let router = service::network_router(core, network.clone(), None);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
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
    let client = http(&ca, Some(&a));
    let other = http(&ca, Some(&b));
    let anonymous = http(&ca, None);
    assert_eq!(
        anonymous
            .get(format!("{endpoint}/v1/network/history/requests"))
            .header("x-moyai-verified-device", &a.receipt.device_id)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let empty: Value = other
        .get(format!("{endpoint}/v1/network/history/requests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty, json!({"requests":[]}));
    let queued: Value = client
        .get(format!("{endpoint}/v1/network/history/requests"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queued["requests"][0]["device_id"], a.receipt.device_id);
    assert_eq!(queued["requests"][0].as_object().unwrap().len(), 7);
    let submission = HistorySubmission {
        request_id: request.request_id.clone(),
        response: Some(HistoryResponse::Detail(HistoryDetail {
            row: detail_row,
            markdown: "\n".repeat(64 * 1024),
            truncated: true,
        })),
        error: None,
    };
    assert!(serde_json::to_vec(&submission).unwrap().len() > 64 * 1024);
    assert_eq!(
        other
            .post(format!("{endpoint}/v1/network/history/results"))
            .json(&submission)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{endpoint}/v1/network/history/results"))
            .header("origin", "https://evil.invalid")
            .json(&submission)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{endpoint}/v1/network/history/results"))
            .json(&submission)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{endpoint}/v1/network/presence"))
            .header("content-type", "application/json")
            .body(format!("{{\"padding\":\"{}\"}}", "x".repeat(70 * 1024)))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    assert_eq!(
        client
            .post(format!("{endpoint}/v1/network/history/results"))
            .header("content-type", "application/json")
            .body("x".repeat(512 * 1024 + 1))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    for path in [
        "/v1/network/history",
        "/v1/network/history/request",
        "/v1/network/history/result",
    ] {
        assert_eq!(
            client
                .get(format!("{endpoint}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        network
            .lock()
            .unwrap()
            .history_result(&request.request_id, f.now)
            .unwrap()
            .status,
        HistoryStatus::Completed
    );
    drop(client);
    drop(other);
    drop(anonymous);
    let _ = stop.send(());
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
