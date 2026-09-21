use super::*;

fn configure(work: &mut SharedWork, actor: &Principal, controller: Option<&str>, runner: &str) {
    let command = ManagementCommand {
        request_id: ulid::Ulid::new().to_string(),
        expected_revision: work.management_snapshot(actor).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: "project".into(),
            label: "Project".into(),
            archived: false,
            members: Some(vec![ProjectMember {
                user_id: actor.user_id.clone(),
                role: ProjectRole::Contributor,
            }]),
            controller_device_ids: controller.into_iter().map(str::to_owned).collect(),
            runner_device_ids: vec![runner.into()],
        },
    };
    work.manage(actor, &command, crate::service::now_ms())
        .unwrap();
}

#[tokio::test]
async fn project_status_uses_current_device_names_for_assignments_environments_and_jobs() {
    let fixture = AdmissionFixture::new();
    let pending = device(&mut fixture.state.network.lock().unwrap(), "未設定の実行PC");
    let session = login(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Json(Login {
            username: "admin".into(),
            password: "test-password-12345".into(),
        }),
    )
    .await
    .unwrap()
    .0;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", session.session.token).parse().unwrap(),
    );
    {
        let mut work = fixture.state.store.lock().unwrap();
        let command = ManagementCommand {
            request_id: "project-pc-names".into(),
            expected_revision: work.management_snapshot(&fixture.actor).unwrap()["revision"]
                .as_str()
                .unwrap()
                .into(),
            request: ManagementRequest::SaveProject {
                id: "project".into(),
                label: "解析プロジェクト".into(),
                archived: false,
                members: Some(vec![ProjectMember {
                    user_id: fixture.actor.user_id.clone(),
                    role: ProjectRole::Contributor,
                }]),
                controller_device_ids: vec![fixture.device.device_id.clone()],
                runner_device_ids: vec![fixture.device.device_id.clone(), pending.id.clone()],
            },
        };
        work.manage(&fixture.actor, &command, crate::service::now_ms())
            .unwrap();
    }
    for label in ["実行PC B", "名称変更後のPC B"] {
        {
            let mut network = fixture.state.network.lock().unwrap();
            let revision = network.revision();
            network
                .set_device(
                    &revision,
                    &fixture.device.device_id,
                    label.into(),
                    BTreeSet::new(),
                    false,
                )
                .unwrap();
        }
        let result = status(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(fixture.device.clone()),
            Query(StatusQuery {
                project_id: "project".into(),
                before: None,
                limit: None,
                environment_before: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["environments"][0]["device_label"], label);
        assert_eq!(json["environments"][0]["label"], "Environment");
        assert_eq!(json["jobs"][0]["device_label"], label);
        let devices = json["execution_devices"].as_array().unwrap();
        let pending = devices
            .iter()
            .find(|row| row["device_id"] == pending.id)
            .unwrap();
        assert_eq!(pending["device_label"], "未設定の実行PC");
        assert_eq!(pending["preparation_state"], "waiting_setup");
        assert_eq!(pending["environment_id"], Value::Null);
        assert_eq!(devices.len(), 2);
    }
}

#[tokio::test]
async fn project_device_gates_cover_human_routes_but_keep_executor_control_and_login_independent() {
    let fixture = AdmissionFixture::new();
    let (pc, ca, certificate, key) = {
        let mut network = fixture.state.network.lock().unwrap();
        let pc = device(&mut network, "Operator PC");
        let ca = network.ca_certificate().to_owned();
        let (certificate, key) = network
            .server_certificate("127.0.0.1".parse().unwrap(), crate::service::now_ms())
            .unwrap();
        (pc, ca, certificate, key)
    };
    configure(
        &mut fixture.state.store.lock().unwrap(),
        &fixture.actor,
        Some(&pc.id),
        &fixture.device.device_id,
    );
    identity::device::bind(
        &fixture.state.store.lock().unwrap().db,
        &fixture.actor,
        &pc.id,
        Some(&fixture.actor.user_id),
        "Operator PC",
        crate::service::now_ms(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("https://{address}/v1/shared");
    let listener = network::tls::NetworkListener::new(
        listener,
        network::tls::acceptor(&certificate, &key, &ca, true).unwrap(),
    )
    .with_authority(address.to_string());
    let app = router(fixture.state.store.clone(), fixture.state.network.clone());
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
    let client = client(&ca, Some(&pc));
    let session = body(
        client
            .post(format!("{endpoint}/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let token = session["token"].as_str().unwrap();
    let job = &fixture.assignment.job.id;
    let input = json!({"request_id":"asset-input","name":"input.txt","content_base64":"aGVsbG8=","sha256":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"});
    let asset = body(
        client
            .post(format!("{endpoint}/projects/project/assets"))
            .bearer_auth(token)
            .json(&input)
            .send()
            .await
            .unwrap(),
    )
    .await;
    body(
        client
            .get(format!("{endpoint}/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    configure(
        &mut fixture.state.store.lock().unwrap(),
        &fixture.actor,
        None,
        &fixture.device.device_id,
    );
    let projects = body(
        client
            .get(format!("{endpoint}/projects"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(projects, json!([]));
    for path in [
        "environments?project_id=project".into(),
        "status?project_id=project".into(),
        "jobs?project_id=project".into(),
        format!("jobs/{job}"),
        format!("jobs/{job}/approval"),
        format!("jobs/{job}/handover"),
        format!("jobs/{job}/assets"),
        format!("jobs/{job}/transcript"),
        format!("assets/{}", asset["id"].as_str().unwrap()),
    ] {
        assert_eq!(
            client
                .get(format!("{endpoint}/{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status(),
            403,
            "GET {path}"
        );
    }
    for (path, payload) in [
        (
            "jobs".into(),
            json!({"request_id":"blocked-job","project_id":"project","environment_id":"environment","title":"blocked","input":{},"descendant_budget":0}),
        ),
        (format!("jobs/{job}/cancel"), json!({})),
        (
            format!("jobs/{job}/approvals/unknown/decision"),
            json!({"decision":"approve"}),
        ),
        (
            format!("jobs/{job}/handover"),
            json!({"expected_revision":1,"new_assignee_id":fixture.actor.user_id}),
        ),
        (
            format!("jobs/{job}/continue"),
            json!({"request_id":"blocked-continue","expected_revision":1,"prompt":"more","input_refs":[]}),
        ),
        ("projects/project/assets".into(), input),
    ] {
        assert_eq!(
            client
                .post(format!("{endpoint}/{path}"))
                .bearer_auth(token)
                .json(&payload)
                .send()
                .await
                .unwrap()
                .status(),
            403,
            "POST {path}"
        );
    }
    // Losing a project role on this PC does not silently sign out its person or
    // prevent the separate executor from finishing its already assigned work.
    body(
        client
            .get(format!("{endpoint}/session"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let device_projects = body(
        client
            .get(format!("{endpoint}/device-projects"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(device_projects["projects"], json!([]));
    assert!(
        attempt(
            State(fixture.state.clone()),
            Extension(fixture.device.clone()),
            Path(fixture.assignment.attempt_id.clone())
        )
        .await
        .is_ok()
    );
    configure(
        &mut fixture.state.store.lock().unwrap(),
        &fixture.actor,
        Some(&pc.id),
        &fixture.device.device_id,
    );
    body(
        client
            .get(format!("{endpoint}/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn project_controller_removed_while_job_read_waits_is_checked_under_the_store_owner() {
    let fixture = AdmissionFixture::new();
    let session = login(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Json(Login {
            username: "admin".into(),
            password: "test-password-12345".into(),
        }),
    )
    .await
    .unwrap()
    .0;
    configure(
        &mut fixture.state.store.lock().unwrap(),
        &fixture.actor,
        Some(&fixture.device.device_id),
        &fixture.device.device_id,
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", session.session.token).parse().unwrap(),
    );
    let mut guard = fixture.state.store.lock().unwrap();
    let mut future = std::pin::pin!(detail(
        State(fixture.state.clone()),
        headers,
        Extension(fixture.device.clone()),
        Path(fixture.assignment.job.id.clone())
    ));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    configure(&mut guard, &fixture.actor, None, &fixture.device.device_id);
    drop(guard);
    assert_eq!(future.await.unwrap_err().0, StatusCode::FORBIDDEN);
}
