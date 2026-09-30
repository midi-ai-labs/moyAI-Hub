use super::*;

#[tokio::test]
async fn runner_environment_catalog_filters_retired_history_only_with_explicit_opt_in() {
    let fixture = AdmissionFixture::new();
    {
        let work = fixture.state.store.lock().unwrap();
        for id in ["retired-folder", "manual-paused"] {
            work.db.execute("INSERT INTO shared_environments(id,label,resource_id,runner_id,enabled) VALUES(?1,?1,'resource',?2,0)", params![id,fixture.device.device_id]).unwrap();
        }
        work.db.execute("INSERT INTO shared_provisioning(environment_id,runner_id,template_id,state,created_at_ms,updated_at_ms) VALUES('retired-folder',?1,'local-folder','ready',1,1)", [&fixture.device.device_id]).unwrap();
        assert_eq!(
            work.runner_environments(&fixture.device.device_id)
                .unwrap()
                .len(),
            3,
            "the existing Rust entry point retains its complete legacy catalog"
        );
        assert_eq!(
            work.current_runner_environments(&fixture.device.device_id)
                .unwrap()
                .len(),
            2
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    // Exercise the real query extractor and current-device operation boundary.
    // This isolated transport supplies the fixture's verified certificate identity.
    let router = provisioning::routes()
        .with_state(fixture.state.clone())
        .layer(Extension(fixture.device.clone()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for (query, expected) in [
        ("", 3),
        ("?current_only=false", 3),
        ("?current_only=true", 2),
    ] {
        let values = body(
            client
                .get(format!("{endpoint}/v1/shared/runner/environments{query}"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        let rows = values.as_array().unwrap();
        assert_eq!(rows.len(), expected, "query {query}");
        assert!(rows.iter().any(|row| row["id"] == "manual-paused"));
        assert_eq!(
            rows.iter().any(|row| row["id"] == "retired-folder"),
            expected == 3
        );
    }
    {
        let work = fixture.state.store.lock().unwrap();
        work.db
            .execute(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<128)
            INSERT INTO shared_environments(id,label,resource_id,runner_id,enabled)
            SELECT 'old-'||x,'Old','resource',?1,0 FROM n",
                [&fixture.device.device_id],
            )
            .unwrap();
        work.db.execute("INSERT INTO shared_provisioning(environment_id,runner_id,template_id,state,created_at_ms,updated_at_ms)
            SELECT id,runner_id,'local-folder','ready',1,1 FROM shared_environments WHERE id LIKE 'old-%'", []).unwrap();
    }
    let legacy = client
        .get(format!("{endpoint}/v1/shared/runner/environments"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        legacy.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "legacy catalog must fail rather than return a partial authority list"
    );
    let current = body(
        client
            .get(format!(
                "{endpoint}/v1/shared/runner/environments?current_only=true"
            ))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(current.as_array().unwrap().len(), 2);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn human_approval_is_routed_to_origin_pc_even_when_executor_has_same_user() {
    let fixture = AdmissionFixture::new();
    let origin = device(&mut fixture.state.network.lock().unwrap(), "WinA 操作PC");
    let origin_identity = DeviceIdentity {
        device_id: origin.id.clone(),
        certificate_sha256: origin.fingerprint,
        address: "127.0.0.1:12346".parse().unwrap(),
    };
    {
        let mut work = fixture.state.store.lock().unwrap();
        let now = crate::service::now_ms();
        identity::device::bind(
            &work.db,
            &fixture.actor,
            &origin.id,
            Some(&fixture.actor.user_id),
            "WinA",
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report("initial-start", ReportOutcome::Started),
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report(
                "initial-finish",
                ReportOutcome::Finished {
                    success: true,
                    result: json!({}),
                    resources_released: true,
                },
            ),
            now,
        )
        .unwrap();
    }
    let origin_session = device_session(
        State(fixture.state.clone()),
        Extension(origin_identity.clone()),
    )
    .await
    .unwrap()
    .0;
    let executor_session = device_session(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
    )
    .await
    .unwrap()
    .0;
    let headers = |token: &str| {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    };
    let job = submit(
        State(fixture.state.clone()),
        headers(&origin_session.token),
        Extension(origin_identity.clone()),
        Json(SubmitJob {
            request_id: "origin-request".into(),
            project_id: "project".into(),
            project_participation: Some(1),
            conversation_id: None,
            revises_job_id: None,
            expected_revised_revision: None,
            environment_id: "environment".into(),
            origin_session_ref: None,
            origin_turn_ref: None,
            origin_turn_epoch: None,
            title: "Operation on WinB".into(),
            input: json!({"prompt":"Create a test file"}),
            descendant_budget: 1,
            start_before_ms: None,
        }),
    )
    .await
    .unwrap()
    .0;
    let assignment = {
        let mut work = fixture.state.store.lock().unwrap();
        let now = crate::service::now_ms();
        let assignment = work.claim(&fixture.device.device_id, now).unwrap().unwrap();
        for (event, outcome) in [
            ("start", ReportOutcome::Started),
            (
                "request",
                ReportOutcome::ApprovalRequested {
                    approval_id: "remote-operation".into(),
                    request: json!({"access":"shell","summary":"Run exact command"}),
                    expires_at_ms: now + 60000,
                },
            ),
        ] {
            work.report(
                &fixture.device.device_id,
                &Report {
                    event_id: event.into(),
                    attempt_id: assignment.attempt_id.clone(),
                    generation: assignment.generation,
                    outcome,
                },
                now,
            )
            .unwrap();
        }
        assignment
    };
    assert_eq!(assignment.job.id, job.id);
    // Same human on another PC may read and rename shared metadata, but cannot
    // stop or delete an active request belonging to the origin PC.
    for who in [&origin_identity, &fixture.device] {
        let token = if who.device_id == origin_identity.device_id {
            &origin_session.token
        } else {
            &executor_session.token
        };
        let view = detail(
            State(fixture.state.clone()),
            headers(token),
            Extension(who.clone()),
            Path(job.id.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            view["can_cancel"],
            who.device_id == origin_identity.device_id
        );
    }
    let rejected = cancel(
        State(fixture.state.clone()),
        headers(&executor_session.token),
        Extension(fixture.device.clone()),
        Path(job.id.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.0, StatusCode::FORBIDDEN);
    let rejected = stop_project_conversation(
        State(fixture.state.clone()),
        headers(&executor_session.token),
        Extension(fixture.device.clone()),
        Path(job.id.clone()),
        Json(StopConversationServices {
            project_id: "project".into(),
            request_id: "other-pc-stop".into(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.0, StatusCode::FORBIDDEN);
    let rejected = delete_conversation(
        State(fixture.state.clone()),
        headers(&executor_session.token),
        Extension(fixture.device.clone()),
        Path(job.id.clone()),
        Json(DeleteConversation {
            project_id: "project".into(),
            request_id: "other-pc-delete".into(),
            expected_revision: 1,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.0, StatusCode::FORBIDDEN);
    let listing = project_conversations(
        State(fixture.state.clone()),
        headers(&executor_session.token),
        Extension(fixture.device.clone()),
        Path("project".into()),
    )
    .await
    .unwrap()
    .0;
    let row = listing["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == job.id)
        .unwrap();
    assert_eq!(row["can_rename"], true);
    assert_eq!(row["can_delete"], false);
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .job(&fixture.actor, &job.id)
            .unwrap()
            .state,
        JobState::Running
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = management::routes()
        .with_state(fixture.state.clone())
        .layer(Extension(origin_identity.clone()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        let response = client
            .request(
                method,
                format!("{endpoint}/v1/shared/jobs/{}/handover", job.id),
            )
            .bearer_auth(&origin_session.token)
            .json(&json!({"expected_revision":job.revision,"new_assignee_id":"obsolete"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        assert!(response.text().await.unwrap().contains("廃止"));
    }
    server.abort();
    let _ = server.await;

    for (who, token, allowed) in [
        (&origin_identity, &origin_session.token, true),
        (&fixture.device, &executor_session.token, false),
    ] {
        let view = approval(
            State(fixture.state.clone()),
            headers(token),
            Extension(who.clone()),
            Path(job.id.clone()),
            Query(ApprovalQuery {
                include_descendants: true,
            }),
        )
        .await
        .unwrap()
        .0
        .unwrap();
        assert_eq!(view.can_decide, allowed);
        assert_eq!(
            view.context.controller_device_label.as_deref(),
            Some("WinA 操作PC")
        );
        assert_eq!(
            view.context.execution_device_label.as_deref(),
            Some("Runner")
        );
        let inbox = fixture
            .state
            .store
            .lock()
            .unwrap()
            .inbox_for_device(
                &fixture.actor,
                None,
                100,
                crate::service::now_ms(),
                Some(&who.device_id),
            )
            .unwrap();
        let item = inbox["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["approval_id"] == "remote-operation")
            .unwrap();
        assert_eq!(item["can_act"], allowed);
        assert_eq!(item["approval_context"]["controller_device_id"], origin.id);
    }
    assert_eq!(
        decide_approval(
            State(fixture.state.clone()),
            headers(&executor_session.token),
            Extension(fixture.device.clone()),
            Path((job.id.clone(), "remote-operation".into())),
            Json(DecideApproval {
                decision: ApprovalDecision::Approve
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    let pending = consume_approval(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Path("remote-operation".into()),
        Json(ConsumeApproval {
            attempt_id: assignment.attempt_id.clone(),
            generation: assignment.generation,
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(pending.is_none());
    let _ = decide_approval(
        State(fixture.state.clone()),
        headers(&origin_session.token),
        Extension(origin_identity),
        Path((job.id, "remote-operation".into())),
        Json(DecideApproval {
            decision: ApprovalDecision::Approve,
        }),
    )
    .await
    .unwrap();
    let answer = consume_approval(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Path("remote-operation".into()),
        Json(ConsumeApproval {
            attempt_id: assignment.attempt_id.clone(),
            generation: assignment.generation,
        }),
    )
    .await
    .unwrap()
    .0
    .unwrap();
    assert!(matches!(
        answer,
        ApprovalConsumeResult::Answer(ApprovalAnswer {
            decision: ApprovalDecision::Approve,
            ..
        })
    ));
    let authorize = || {
        let runner = fixture.device.device_id.clone();
        let assignment = assignment.clone();
        operation(
            fixture.state.clone(),
            fixture.device.clone(),
            DeviceAccess::Effect {
                attempt_id: assignment.attempt_id.clone(),
            },
            move |store| {
                store.authorize_effect(
                    &runner,
                    &assignment.attempt_id,
                    assignment.generation,
                    assignment.authority_generation,
                    Some("remote-operation"),
                    crate::service::now_ms(),
                )
            },
        )
    };
    // No Desktop heartbeat/presence is required while its accepted work runs.
    let _ = authorize().await.unwrap();
    let guard = fixture.state.store.lock().unwrap();
    let mut pending_effect = std::pin::pin!(authorize());
    std::future::poll_fn(|cx| {
        assert!(pending_effect.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    {
        let mut network = fixture.state.network.lock().unwrap();
        let revision = network.revision();
        network
            .set_device(
                &revision,
                &origin.id,
                "WinA 操作PC".into(),
                BTreeSet::new(),
                true,
            )
            .unwrap();
    }
    drop(guard);
    assert_eq!(pending_effect.await.unwrap_err().0, StatusCode::FORBIDDEN);
    assert_eq!(
        consume_approval(
            State(fixture.state.clone()),
            Extension(fixture.device.clone()),
            Path("remote-operation".into()),
            Json(ConsumeApproval {
                attempt_id: assignment.attempt_id,
                generation: assignment.generation
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
}

fn configure(work: &mut SharedWork, actor: &Principal, controller: Option<&str>, runner: &str) {
    let command = ManagementCommand {
        request_id: ulid::Ulid::new().to_string(),
        expected_revision: work.management_snapshot(actor).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            overview: None,
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

#[test]
fn stopping_or_revoking_an_approver_pc_retires_delivered_approval_on_another_executor() {
    for command in ["hub_network_admission", "hub_network_device"] {
        let fixture = AdmissionFixture::new();
        let approver = device(&mut fixture.state.network.lock().unwrap(), "Approval PC");
        let now = crate::service::now_ms();
        let mut work = fixture.state.store.lock().unwrap();
        // This controller originally submitted the work; the separate executor
        // never gains authority to approve it.
        // AdmissionFixture uses the device-less storage helper, so its saved
        // participation generation is NULL. Model the legacy controller
        // admission as a complete pair, exactly as submit_for_device does.
        work.db.execute("UPDATE shared_jobs SET controller_device_id=?1,controller_participation_generation=1 WHERE id=?2",
            params![approver.id, fixture.assignment.job.id]).unwrap();
        identity::device::bind(
            &work.db,
            &fixture.actor,
            &approver.id,
            Some(&fixture.actor.user_id),
            "Approval PC",
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report("start", ReportOutcome::Started),
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report(
                "request",
                ReportOutcome::ApprovalRequested {
                    approval_id: "approval".into(),
                    request: json!({"operation":"exact command"}),
                    expires_at_ms: now + 60000,
                },
            ),
            now,
        )
        .unwrap();
        work.decide_approval_for_device(
            &fixture.actor,
            &fixture.assignment.job.id,
            "approval",
            ApprovalDecision::Approve,
            now,
            Some(&approver.id),
        )
        .unwrap();
        work.consume_approval(
            &fixture.device.device_id,
            "approval",
            &fixture.assignment.attempt_id,
            fixture.assignment.generation,
            now,
        )
        .unwrap();
        assert!(
            work.authorize_effect(
                &fixture.device.device_id,
                &fixture.assignment.attempt_id,
                fixture.assignment.generation,
                fixture.assignment.authority_generation,
                Some("approval"),
                now
            )
            .is_ok()
        );
        let mut network = fixture.state.network.lock().unwrap();
        let args = json!({"expectedRevision":network.revision(),"deviceId":approver.id,"stopped":true,"revoked":true,"label":"Approval PC","groups":[]});
        crate::admin::dispatch_network_locked(&work, &mut network, command, &args).unwrap();
        assert!(
            network
                .admission_revision(&fixture.device.device_id, now)
                .is_ok()
        );
        assert!(
            work.consume_approval(
                &fixture.device.device_id,
                "approval",
                &fixture.assignment.attempt_id,
                fixture.assignment.generation,
                now
            )
            .is_err()
        );
        assert!(
            work.authorize_effect(
                &fixture.device.device_id,
                &fixture.assignment.attempt_id,
                fixture.assignment.generation,
                fixture.assignment.authority_generation,
                Some("approval"),
                now
            )
            .is_err()
        );
    }
}

#[test]
fn registry_does_not_stop_a_pc_when_approval_invalidation_cannot_commit() {
    let fixture = AdmissionFixture::new();
    let work = fixture.state.store.lock().unwrap();
    work.db.execute_batch("CREATE TRIGGER approval_invalidation_failure BEFORE UPDATE OF authorization_generation ON identity_users BEGIN SELECT RAISE(ABORT,'controlled failure'); END;").unwrap();
    let mut network = fixture.state.network.lock().unwrap();
    let revision = network.revision();
    let args =
        json!({"expectedRevision":revision,"deviceId":fixture.device.device_id,"stopped":true});
    assert!(
        crate::admin::dispatch_network_locked(&work, &mut network, "hub_network_admission", &args)
            .is_err()
    );
    assert_eq!(network.revision(), revision);
    assert!(
        network
            .admission_revision(&fixture.device.device_id, crate::service::now_ms())
            .is_ok()
    );
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
                overview: None,
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
        let capabilities = ManagementCommand {
            request_id: "project-pc-capabilities".into(),
            expected_revision: work.management_snapshot(&fixture.actor).unwrap()["revision"]
                .as_str()
                .unwrap()
                .into(),
            request: ManagementRequest::SetEnvironmentCapabilities {
                environment_id: "environment".into(),
                names: vec!["Gurobi".into()],
            },
        };
        work.manage(&fixture.actor, &capabilities, crate::service::now_ms())
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
        let environment = json["environments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "environment")
            .unwrap();
        assert_eq!(environment["device_label"], label);
        assert_eq!(environment["label"], "Environment");
        assert_eq!(json["jobs"][0]["device_label"], label);
        let devices = json["execution_devices"].as_array().unwrap();
        let pending = devices
            .iter()
            .find(|row| row["device_id"] == pending.id)
            .unwrap();
        assert_eq!(pending["device_label"], "未設定の実行PC");
        assert_eq!(pending["preparation_state"], "waiting_local_folder");
        assert!(pending["environment_id"].is_string());
        assert_eq!(devices.len(), 2);
        let catalog = environments(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(fixture.device.clone()),
            Query(ProjectQuery {
                project_id: "project".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(catalog[0]["device_id"], fixture.device.device_id);
        assert_eq!(catalog[0]["device_label"], label);
        assert_eq!(catalog[0]["capabilities"][0]["name"], "Gurobi");
        assert_eq!(catalog[0]["capabilities"][0]["source"], "declared");
        assert_eq!(catalog[0]["capabilities"][0]["observed_at_ms"], Value::Null);
        assert_eq!(catalog[0]["can_submit"], true);
        assert!(catalog[0]["runner_contact"].is_object());
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
            if path.ends_with("/handover") {
                410
            } else {
                403
            },
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
            if path.ends_with("/handover") {
                410
            } else {
                403
            },
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
            {
                let mut headers = HeaderMap::new();
                headers.insert(
                    "x-moyai-runner-capabilities",
                    store::MULTI_DEVICE_SESSION_CAPABILITY.parse().unwrap(),
                );
                headers
            },
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
