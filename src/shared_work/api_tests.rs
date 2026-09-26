use super::*;
use crate::network::{self, DeviceNetwork};
use std::collections::BTreeSet;
#[path = "api_tests/project_devices.rs"]
mod project_devices;
#[path = "api_tests/remembered.rs"]
mod remembered;

struct Device {
    id: String,
    certificate: String,
    key: String,
    fingerprint: String,
}
fn device(network: &mut DeviceNetwork, label: &str) -> Device {
    let key = rcgen::KeyPair::generate().unwrap();
    let csr = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .serialize_request(&key)
        .unwrap()
        .pem()
        .unwrap();
    let invite = network
        .invite(
            &network.revision(),
            label.into(),
            BTreeSet::new(),
            crate::service::now_ms(),
        )
        .unwrap();
    let receipt = network
        .enroll(
            invite["code"].as_str().unwrap(),
            &csr,
            "127.0.0.1".parse().unwrap(),
            crate::service::now_ms(),
        )
        .unwrap();
    Device {
        id: receipt.device_id,
        certificate: receipt.certificate_pem,
        key: key.serialize_pem(),
        fingerprint: receipt.certificate_sha256,
    }
}
fn client(ca: &str, device: Option<&Device>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap());
    if let Some(device) = device {
        builder = builder.identity(
            reqwest::Identity::from_pem(
                format!("{}\n{}", device.certificate, device.key).as_bytes(),
            )
            .unwrap(),
        );
    }
    builder.build().unwrap()
}
async fn body(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(status, 200, "{body}");
    body
}

struct AdmissionFixture {
    _directory: tempfile::TempDir,
    state: ApiState,
    device: DeviceIdentity,
    actor: Principal,
    assignment: Assignment,
}
impl AdmissionFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut network =
            DeviceNetwork::open(&directory.path().join("network.json"), "admission-race").unwrap();
        let worker = device(&mut network, "Runner");
        let mut work = SharedWork::open(&directory.path().join("work.sqlite3")).unwrap();
        let now = crate::service::now_ms();
        let actor =
            identity::bootstrap_admin(&work.db, "admin", "管理者", "test-password-12345", now)
                .unwrap();
        identity::device::bind(
            &work.db,
            &actor,
            &worker.id,
            Some(&actor.user_id),
            "Runner",
            now,
        )
        .unwrap();
        work.create_project(&actor, "project", "Project").unwrap();
        identity::set_project_role(
            &work.db,
            &actor,
            "project",
            &actor.user_id,
            Some(ProjectRole::Contributor),
            now,
        )
        .unwrap();
        work.register_environment(
            &actor,
            &Environment {
                id: "environment".into(),
                label: "Environment".into(),
                resource_id: "resource".into(),
                runner_id: worker.id.clone(),
                capacity: 1,
                project_ids: vec!["project".into()],
            },
        )
        .unwrap();
        work.submit(
            &actor,
            &SubmitJob {
                request_id: "request".into(),
                project_id: "project".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "environment".into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "Work".into(),
                input: json!({}),
                descendant_budget: 1,
                start_before_ms: None,
            },
            now,
        )
        .unwrap();
        let assignment = work.claim(&worker.id, now).unwrap().unwrap();
        Self {
            _directory: directory,
            state: ApiState {
                store: Arc::new(Mutex::new(work)),
                network: Arc::new(Mutex::new(network)),
                slots: Arc::new(tokio::sync::Semaphore::new(8)),
                login_slots: Arc::new(tokio::sync::Semaphore::new(2)),
                web_access: None,
            },
            device: DeviceIdentity {
                device_id: worker.id,
                certificate_sha256: worker.fingerprint,
                address: "127.0.0.1:12345".parse().unwrap(),
            },
            actor,
            assignment,
        }
    }
    fn report(&self, event: &str, outcome: ReportOutcome) -> Report {
        Report {
            event_id: event.into(),
            attempt_id: self.assignment.attempt_id.clone(),
            generation: self.assignment.generation,
            outcome,
        }
    }
    async fn after_device_change<T>(
        &self,
        revoke: bool,
        future: impl std::future::Future<Output = ApiResult<T>>,
    ) -> ApiResult<T> {
        // Poll the actual handler through its synchronous checks until it must wait
        // for the store. No sleep or scheduler timing decides the interleaving.
        let guard = self.state.store.lock().unwrap();
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        {
            let mut network = self.state.network.lock().unwrap();
            let revision = network.revision();
            if revoke {
                network
                    .set_device(
                        &revision,
                        &self.device.device_id,
                        "Runner".into(),
                        BTreeSet::new(),
                        true,
                    )
                    .unwrap();
            } else {
                network
                    .set_admission(&revision, &self.device.device_id, true)
                    .unwrap();
            }
        }
        drop(guard);
        future.await
    }
}

#[tokio::test]
async fn runner_data_http_routes_stage_child_input_and_scope_child_artifacts() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let fixture = AdmissionFixture::new();
    let app = super::data::routes()
        .layer(Extension(fixture.device.clone()))
        .with_state(fixture.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let parent = fixture.assignment.clone();
    {
        let mut work = fixture.state.store.lock().unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report("http-parent-start", ReportOutcome::Started),
            crate::service::now_ms(),
        )
        .unwrap();
    }
    let content = b"print('hello')\n";
    let input = body(
        client
            .post(format!(
                "{endpoint}/v1/shared/runner/attempts/{}/assets",
                parent.attempt_id
            ))
            .json(
                &json!({"generation":parent.generation,"kind":"input","base_sha256":null,
            "upload":{"request_id":"http-child-input","name":"src/app.py",
            "content_base64":base64::engine::general_purpose::STANDARD.encode(content),
            "sha256":format!("{:x}",Sha256::digest(content))}}),
            )
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(input["kind"], "input");
    assert_eq!(input["job_id"], parent.job.id);
    let child = {
        let mut work = fixture.state.store.lock().unwrap();
        work.report(&fixture.device.device_id,&fixture.report("http-parent-yield",ReportOutcome::YieldToChild {
            checkpoint:json!({"version":1}),child_environment_id:"environment".into(),
            child_title:"Child".into(),child_input:json!({"version":2,"prompt":"Use input","input_refs":[input["id"]]}),
            resources_released:true,
        }),crate::service::now_ms()).unwrap();
        let child = work
            .claim(&fixture.device.device_id, crate::service::now_ms())
            .unwrap()
            .unwrap();
        work.report(
            &fixture.device.device_id,
            &Report {
                event_id: "http-child-start".into(),
                attempt_id: child.attempt_id.clone(),
                generation: child.generation,
                outcome: ReportOutcome::Started,
            },
            crate::service::now_ms(),
        )
        .unwrap();
        child
    };
    let child_input = body(
        client
            .get(format!(
                "{endpoint}/v1/shared/runner/attempts/{}/assets/{}",
                child.attempt_id,
                input["id"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(child_input["asset"]["id"], input["id"]);
    let bytes = b"child output";
    let artifact = body(
        client
            .post(format!(
                "{endpoint}/v1/shared/runner/attempts/{}/assets",
                child.attempt_id
            ))
            .json(
                &json!({"generation":child.generation,"kind":"artifact","base_sha256":null,
            "upload":{"request_id":"http-child-artifact","name":"result.txt",
            "content_base64":base64::engine::general_purpose::STANDARD.encode(bytes),
            "sha256":format!("{:x}",Sha256::digest(bytes))}}),
            )
            .send()
            .await
            .unwrap(),
    )
    .await;
    let resumed = {
        let mut work = fixture.state.store.lock().unwrap();
        work.report(
            &fixture.device.device_id,
            &Report {
                event_id: "http-child-done".into(),
                attempt_id: child.attempt_id.clone(),
                generation: child.generation,
                outcome: ReportOutcome::Finished {
                    success: true,
                    result: json!({"done":true}),
                    resources_released: true,
                },
            },
            crate::service::now_ms(),
        )
        .unwrap();
        work.claim(&fixture.device.device_id, crate::service::now_ms())
            .unwrap()
            .unwrap()
    };
    let list_url = format!(
        "{endpoint}/v1/shared/runner/attempts/{}/children/{}/artifacts?generation={}",
        resumed.attempt_id, child.job.id, resumed.generation
    );
    let listed = body(client.get(&list_url).send().await.unwrap()).await;
    assert_eq!(listed["items"][0]["id"], artifact["id"]);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    let download_url = format!(
        "{endpoint}/v1/shared/runner/attempts/{}/children/{}/artifacts/{}?generation={}",
        resumed.attempt_id,
        child.job.id,
        artifact["id"].as_str().unwrap(),
        resumed.generation
    );
    let downloaded = body(client.get(&download_url).send().await.unwrap()).await;
    assert_eq!(downloaded["asset"]["id"], artifact["id"]);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(downloaded["content_base64"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    assert_eq!(
        client
            .get(format!("{list_url}0"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    server.abort();
}

#[tokio::test]
async fn origin_lookup_route_requires_bound_session_and_keeps_independent_pages() {
    let fixture = AdmissionFixture::new();
    let job = {
        let mut work = fixture.state.store.lock().unwrap();
        work.submit_for_device(
            &fixture.actor,
            &SubmitJob {
                request_id: "origin-route".into(),
                project_id: "project".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "environment".into(),
                origin_session_ref: Some("private-chat".into()),
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "From chat".into(),
                input: json!({"prompt":"test"}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            crate::service::now_ms(),
            Some(&fixture.device.device_id),
        )
        .unwrap()
    };
    let input = || OriginWorkQuery {
        job_before: None,
        service_before: None,
        include_jobs: None,
        include_services: None,
    };
    assert_eq!(
        origin_work(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Extension(fixture.device.clone()),
            Path("private-chat".into()),
            Query(input()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    let session = device_session(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
    )
    .await
    .unwrap()
    .0;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", session.token).parse().unwrap(),
    );
    let result = origin_work(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path("private-chat".into()),
        Query(input()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].project_id, "project");
    assert_eq!(result.jobs[0].job.id, job.id);
    assert_eq!(result.jobs[0].job.conversation_id, job.conversation_id);
    assert!(result.retained_services.is_empty());
    let no_jobs = origin_work(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path("private-chat".into()),
        Query(OriginWorkQuery {
            include_jobs: Some(false),
            ..input()
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(no_jobs.jobs.is_empty());
    let wrong_device = DeviceIdentity {
        device_id: "foreign".into(),
        ..fixture.device.clone()
    };
    assert_eq!(
        origin_work(
            State(fixture.state.clone()),
            headers,
            Extension(wrong_device),
            Path("private-chat".into()),
            Query(input()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn origin_stop_and_history_routes_require_the_bound_device_and_fence_submit() {
    let fixture = AdmissionFixture::new();
    let session = device_session(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
    )
    .await
    .unwrap()
    .0;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", session.token).parse().unwrap(),
    );
    let admit = || OriginTurnAdmit {
        request_id: "turn-admit".into(),
        create: true,
        origin_turn_revision: 1,
    };
    assert_eq!(
        origin_turn_admit(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Extension(fixture.device.clone()),
            Path(("chat".into(), "turn-1".into())),
            Json(admit())
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    let wrong = DeviceIdentity {
        device_id: "foreign".into(),
        ..fixture.device.clone()
    };
    assert_eq!(
        origin_turn_admit(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(wrong),
            Path(("chat".into(), "turn-1".into())),
            Json(admit())
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    let admitted = origin_turn_admit(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path(("chat".into(), "turn-1".into())),
        Json(admit()),
    )
    .await
    .unwrap()
    .0;
    let job = submit(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Json(SubmitJob {
            request_id: "turn-job".into(),
            project_id: "project".into(),
            project_participation: None,
            conversation_id: None,
            revises_job_id: None,
            expected_revised_revision: None,
            environment_id: "environment".into(),
            origin_session_ref: Some("chat".into()),
            origin_turn_ref: Some("turn-1".into()),
            origin_turn_epoch: admitted["epoch"].as_u64(),
            title: "Turn job".into(),
            input: json!({"prompt":"test"}),
            descendant_budget: 0,
            start_before_ms: None,
        }),
    )
    .await
    .unwrap()
    .0;
    let page = conversation_history(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path(job.id.clone()),
        Query(ConversationHistoryQuery {
            project_id: "project".into(),
            snapshot: None,
            before: None,
            limit: Some(1),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(page.jobs[0].job.id, job.id);
    let wrong = DeviceIdentity {
        device_id: "foreign".into(),
        ..fixture.device.clone()
    };
    assert_eq!(
        conversation_history(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(wrong),
            Path(job.id.clone()),
            Query(ConversationHistoryQuery {
                project_id: "project".into(),
                snapshot: None,
                before: None,
                limit: Some(1)
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    let accepted = origin_turn_stop(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path(("chat".into(), "turn-1".into())),
        Json(OriginTurnStop {
            request_id: "stop-turn".into(),
            origin_turn_revision: 1,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .job(&fixture.actor, &job.id)
            .unwrap()
            .state,
        JobState::Cancelled
    );
    assert_eq!(
        submit(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(fixture.device.clone()),
            Json(SubmitJob {
                request_id: "late-turn-job".into(),
                project_id: "project".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "environment".into(),
                origin_session_ref: Some("chat".into()),
                origin_turn_ref: Some("turn-1".into()),
                origin_turn_epoch: admitted["epoch"].as_u64(),
                title: "Late".into(),
                input: json!({}),
                descendant_budget: 0,
                start_before_ms: None,
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let stopped = origin_stop(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path("chat".into()),
        Json(OriginStop {
            request_id: "stop-all".into(),
            through_revision: 1,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(stopped["origin_turn_ref"], Value::Null);
    assert_eq!(
        origin_turn_admit(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(fixture.device.clone()),
            Path(("chat".into(), "delayed-turn".into())),
            Json(OriginTurnAdmit {
                request_id: "delayed".into(),
                create: true,
                origin_turn_revision: 1
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        origin_turn_admit(
            State(fixture.state.clone()),
            headers,
            Extension(fixture.device.clone()),
            Path(("chat".into(), "next-turn".into())),
            Json(OriginTurnAdmit {
                request_id: "next".into(),
                create: true,
                origin_turn_revision: 2
            })
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn runner_capability_header_gates_claim_poll_and_attempt_detail() {
    let fixture = AdmissionFixture::new();
    let job_id = fixture.assignment.job.id.clone();
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability=?2 WHERE id=?1",
            rusqlite::params![job_id, store::MULTI_DEVICE_SESSION_CAPABILITY],
        )
        .unwrap();
    let scope = || AssignmentScope {
        environment_ids: None,
    };
    let old_poll = assignments(
        State(fixture.state.clone()),
        HeaderMap::new(),
        Extension(fixture.device.clone()),
        Query(scope()),
    )
    .await
    .unwrap()
    .0;
    assert!(old_poll.is_empty());
    assert!(
        claim(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Extension(fixture.device.clone()),
            None,
        )
        .await
        .unwrap()
        .0
        .is_none()
    );
    assert_eq!(
        attempt(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Extension(fixture.device.clone()),
            Path(fixture.assignment.attempt_id.clone()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::NOT_FOUND
    );
    let mut capable = HeaderMap::new();
    capable.insert(
        "x-moyai-runner-capabilities",
        store::MULTI_DEVICE_SESSION_CAPABILITY.parse().unwrap(),
    );
    let poll = assignments(
        State(fixture.state.clone()),
        capable.clone(),
        Extension(fixture.device.clone()),
        Query(scope()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(poll.len(), 1);
    assert_eq!(
        poll[0].required_runner_capabilities,
        [store::MULTI_DEVICE_SESSION_CAPABILITY]
    );
    assert_eq!(
        claim(
            State(fixture.state.clone()),
            capable.clone(),
            Extension(fixture.device.clone()),
            None,
        )
        .await
        .unwrap()
        .0
        .unwrap()
        .attempt_id,
        fixture.assignment.attempt_id
    );
    assert_eq!(
        attempt(
            State(fixture.state.clone()),
            capable,
            Extension(fixture.device.clone()),
            Path(fixture.assignment.attempt_id.clone()),
        )
        .await
        .unwrap()
        .0
        .assignment
        .attempt_id,
        fixture.assignment.attempt_id
    );
}

#[tokio::test]
async fn runner_service_stop_route_requires_current_attempt_generation() {
    let fixture = AdmissionFixture::new();
    let now = crate::service::now_ms();
    {
        let mut work = fixture.state.store.lock().unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report("started", ReportOutcome::Started),
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report(
                "retain",
                ReportOutcome::RetainService {
                    service_id: "preview".into(),
                    expires_at_ms: now + 60_000,
                },
            ),
            now,
        )
        .unwrap();
    }
    let stale = conversation_service_stop(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Path("preview".into()),
        Json(ConversationServiceStop {
            attempt_id: fixture.assignment.attempt_id.clone(),
            generation: fixture.assignment.generation + 1,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.0, StatusCode::FORBIDDEN);
    let stopped = conversation_service_stop(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Path("preview".into()),
        Json(ConversationServiceStop {
            attempt_id: fixture.assignment.attempt_id.clone(),
            generation: fixture.assignment.generation,
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(stopped.stop_requested);
    assert!(stopped.can_stop);
    let lease = runner_services(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(lease.len(), 1);
    assert!(lease[0].stop_requested);
    let scoped = conversation_services_for_attempt(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Query(ConversationServicesQuery {
            attempt_id: fixture.assignment.attempt_id.clone(),
            generation: fixture.assignment.generation,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].service_id, "preview");
    let stale_discovery = conversation_services_for_attempt(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
        Query(ConversationServicesQuery {
            attempt_id: fixture.assignment.attempt_id.clone(),
            generation: fixture.assignment.generation + 1,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale_discovery.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn conversation_stop_handler_requires_device_bound_person_and_project_control() {
    let fixture = AdmissionFixture::new();
    let now = crate::service::now_ms();
    {
        let mut work = fixture.state.store.lock().unwrap();
        let command = ManagementCommand {
            request_id: "enable-controller-for-stop".into(),
            expected_revision: work.management_snapshot(&fixture.actor).unwrap()["revision"]
                .as_str()
                .unwrap()
                .into(),
            request: ManagementRequest::SaveProject {
                id: "project".into(),
                label: "Project".into(),
                archived: false,
                members: Some(vec![ProjectMember {
                    user_id: fixture.actor.user_id.clone(),
                    role: ProjectRole::Manager,
                }]),
                controller_device_ids: vec![fixture.device.device_id.clone()],
                runner_device_ids: vec![fixture.device.device_id.clone()],
            },
        };
        work.manage(&fixture.actor, &command, now).unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report("started", ReportOutcome::Started),
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report(
                "retained",
                ReportOutcome::RetainService {
                    service_id: "api-preview".into(),
                    expires_at_ms: now + 60_000,
                },
            ),
            now,
        )
        .unwrap();
    }
    let session = device_session(
        State(fixture.state.clone()),
        Extension(fixture.device.clone()),
    )
    .await
    .unwrap()
    .0;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", session.token).parse().unwrap(),
    );
    let conversation_id = fixture.assignment.job.conversation_id.clone();
    let input = || StopConversationServices {
        project_id: "project".into(),
        request_id: "stop-api-request".into(),
    };
    assert_eq!(
        stop_conversation_services(
            State(fixture.state.clone()),
            HeaderMap::new(),
            Extension(fixture.device.clone()),
            Path(conversation_id.clone()),
            Json(input()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        stop_conversation_services(
            State(fixture.state.clone()),
            headers.clone(),
            Extension(fixture.device.clone()),
            Path("other-conversation".into()),
            Json(input()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::NOT_FOUND
    );
    let first = stop_conversation_services(
        State(fixture.state.clone()),
        headers.clone(),
        Extension(fixture.device.clone()),
        Path(conversation_id.clone()),
        Json(input()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first["accepted"], true);
    assert_eq!(first["conversation_id"], conversation_id);
    assert_eq!(
        stop_conversation_services(
            State(fixture.state.clone()),
            headers,
            Extension(fixture.device.clone()),
            Path(conversation_id),
            Json(input()),
        )
        .await
        .unwrap()
        .0,
        first
    );
    assert!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .runner_services(&fixture.device.device_id, now)
            .unwrap()[0]
            .stop_requested
    );
}

#[tokio::test]
async fn runner_contact_is_not_inferred_from_a_fresh_desktop_job_read() {
    let fixture = AdmissionFixture::new();
    let mut work = fixture.state.store.lock().unwrap();
    work.report(
        &fixture.device.device_id,
        &fixture.report("started", ReportOutcome::Started),
        crate::service::now_ms(),
    )
    .unwrap();
    // Direct storage setup intentionally supplies no observed Runner HTTP contact.
    // A new Desktop projection must distinguish this from a recent response.
    let detail = work
        .job_detail(&fixture.actor, &fixture.assignment.job.id)
        .unwrap();
    assert_eq!(detail["state"], "running");
    assert_eq!(
        detail["runner_contact"]["state"], "unconfirmed",
        "fetching a job does not confirm communication with its Runner"
    );
    assert!(detail["wait_reason"].as_str().unwrap().contains("通信"));
    assert_eq!(
        work.assignments(&fixture.device.device_id).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn device_stop_while_started_waits_for_store_prevents_start() {
    let fixture = AdmissionFixture::new();
    let result = fixture
        .after_device_change(
            false,
            report(
                State(fixture.state.clone()),
                Extension(fixture.device.clone()),
                Json(fixture.report("started", ReportOutcome::Started)),
            ),
        )
        .await;
    assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .job(&fixture.actor, &fixture.assignment.job.id)
            .unwrap()
            .state,
        JobState::Assigned
    );
}

#[tokio::test]
async fn device_revocation_while_started_waits_for_store_prevents_start() {
    let fixture = AdmissionFixture::new();
    let result = fixture
        .after_device_change(
            true,
            report(
                State(fixture.state.clone()),
                Extension(fixture.device.clone()),
                Json(fixture.report("started", ReportOutcome::Started)),
            ),
        )
        .await;
    assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .job(&fixture.actor, &fixture.assignment.job.id)
            .unwrap()
            .state,
        JobState::Assigned
    );
}

#[tokio::test]
async fn device_stop_while_approval_delivery_waits_for_store_preserves_undelivered_answer() {
    let fixture = AdmissionFixture::new();
    {
        let mut work = fixture.state.store.lock().unwrap();
        let now = crate::service::now_ms();
        work.report(
            &fixture.device.device_id,
            &fixture.report("started", ReportOutcome::Started),
            now,
        )
        .unwrap();
        work.report(
            &fixture.device.device_id,
            &fixture.report(
                "approval",
                ReportOutcome::ApprovalRequested {
                    approval_id: "approval".into(),
                    request: json!({"tool":"shell","arguments":{"command":"controlled effect"}}),
                    expires_at_ms: now + 60_000,
                },
            ),
            now,
        )
        .unwrap();
        work.decide_approval(
            &fixture.actor,
            &fixture.assignment.job.id,
            "approval",
            ApprovalDecision::Approve,
            now,
        )
        .unwrap();
    }
    let result = fixture
        .after_device_change(
            false,
            consume_approval(
                State(fixture.state.clone()),
                Extension(fixture.device.clone()),
                Path("approval".into()),
                Json(ConsumeApproval {
                    attempt_id: fixture.assignment.attempt_id.clone(),
                    generation: fixture.assignment.generation,
                }),
            ),
        )
        .await;
    assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .approval(
                &fixture.actor,
                &fixture.assignment.job.id,
                crate::service::now_ms()
            )
            .unwrap()
            .unwrap()
            .status,
        "decided"
    );
}

#[tokio::test]
async fn real_tls_requires_person_and_device_and_preserves_job_across_client_connections() {
    let dir = tempfile::tempdir().unwrap();
    let mut network = DeviceNetwork::open(&dir.path().join("devices.json"), "shared-test").unwrap();
    let desktop = device(&mut network, "Desktop A");
    let other = device(&mut network, "Desktop B");
    let worker = device(&mut network, "Runner E");
    let ca = network.ca_certificate().to_owned();
    let (certificate, key) = network
        .server_certificate("127.0.0.1".parse().unwrap(), crate::service::now_ms())
        .unwrap();
    let mut work = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
    let now = crate::service::now_ms();
    let admin =
        identity::bootstrap_admin(&work.db, "admin", "管理者", "test-password-12345", now).unwrap();
    let user = identity::create_user(
        &work.db,
        &admin,
        "alice",
        "山田",
        "test-password-12345",
        false,
        now,
    )
    .unwrap();
    work.create_project(&admin, "alpha", "Project Alpha")
        .unwrap();
    identity::set_project_role(
        &work.db,
        &admin,
        "alpha",
        &user.user_id,
        Some(ProjectRole::Contributor),
        now,
    )
    .unwrap();
    work.register_environment(
        &admin,
        &Environment {
            id: "solver".into(),
            label: "Gurobi".into(),
            resource_id: "wine".into(),
            runner_id: worker.id.clone(),
            capacity: 1,
            project_ids: vec!["alpha".into()],
        },
    )
    .unwrap();
    // A returning client must observe expiry even if the Runner is still offline.
    let old_request = SubmitJob {
        request_id: "offline-expired".into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: "solver".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "Offline request".into(),
        input: json!({"version":1,"prompt":"old"}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    let old_job = work
        .submit(&user, &old_request, now - DEFAULT_START_WINDOW_MS - 1)
        .unwrap();
    identity::device::bind(
        &work.db,
        &admin,
        &desktop.id,
        Some(&user.user_id),
        "Desktop",
        now,
    )
    .unwrap();
    let store = Arc::new(Mutex::new(work));
    let network = Arc::new(Mutex::new(network));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("https://{address}");
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
        .unwrap()
    });
    let anonymous = client(&ca, None);
    assert_eq!(
        anonymous
            .post(format!("{endpoint}/v1/shared/device-session"))
            .header("x-moyai-verified-device", &desktop.id)
            .json(&json!({"username":"alice","password":"test-password-12345"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let a = client(&ca, Some(&desktop));
    assert_eq!(
        a.get(format!("{endpoint}/v1/shared/projects"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let session = body(
        a.post(format!("{endpoint}/v1/shared/device-session"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let token = session["token"].as_str().unwrap();
    let expired = body(
        a.get(format!("{endpoint}/v1/shared/jobs/{}", old_job.id))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(expired["state"], "failed");
    assert_eq!(expired["result"]["error"], "start_deadline_exceeded");
    assert!(
        expired["wait_reason"]
            .as_str()
            .unwrap()
            .contains("開始期限")
    );
    assert_eq!(expired["start_before_ms"], old_job.start_before_ms);
    let authenticated_session = body(
        a.get(format!("{endpoint}/v1/shared/session"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(authenticated_session["principal"]["user_id"], user.user_id);
    assert_eq!(
        authenticated_session["expires_at_ms"],
        session["expires_at_ms"]
    );
    let b = client(&ca, Some(&other));
    assert_eq!(
        b.get(format!("{endpoint}/v1/shared/projects"))
            .bearer_auth(token)
            .header("x-moyai-verified-device", &desktop.id)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        a.get(format!("{endpoint}/v1/shared/projects"))
            .bearer_auth(token)
            .header("Origin", "https://attacker.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // Data routes have their own body limit: real mTLS must admit this payload, while
    // retaining device-bound human authorization and an immutable retry identity.
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let input_bytes = vec![173u8; 1024 * 1024];
    let asset_request = json!({"request_id":"large-input","name":"data.bin","content_base64":STANDARD.encode(&input_bytes),"sha256":format!("{:x}",Sha256::digest(&input_bytes))});
    let asset = body(
        a.post(format!("{endpoint}/v1/shared/projects/alpha/assets"))
            .bearer_auth(token)
            .json(&asset_request)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(asset["byte_length"], input_bytes.len());
    let retry = body(
        a.post(format!("{endpoint}/v1/shared/projects/alpha/assets"))
            .bearer_auth(token)
            .json(&asset_request)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(retry["id"], asset["id"]);
    let asset_url = format!(
        "{endpoint}/v1/shared/assets/{}",
        asset["id"].as_str().unwrap()
    );
    let downloaded = body(a.get(&asset_url).bearer_auth(token).send().await.unwrap()).await;
    assert_eq!(
        STANDARD
            .decode(downloaded["content_base64"].as_str().unwrap())
            .unwrap(),
        input_bytes
    );
    assert_eq!(
        b.get(&asset_url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let request = json!({"request_id":"desktop-request-1","project_id":"alpha","environment_id":"solver","title":"共有計算","input":{"version":2,"prompt":"Use the shared file","input_refs":[asset["id"]]},"descendant_budget":0});
    let accepted = body(
        a.post(format!("{endpoint}/v1/shared/jobs"))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let job = accepted["id"].as_str().unwrap().to_owned();
    assert_eq!(
        accepted["start_before_ms"].as_u64().unwrap(),
        accepted["created_at_ms"].as_u64().unwrap() + DEFAULT_START_WINDOW_MS
    );
    let mut expired_request = request.clone();
    expired_request["request_id"] = json!("explicit-expired");
    expired_request["start_before_ms"] = json!(now - 1);
    assert_eq!(
        a.post(format!("{endpoint}/v1/shared/jobs"))
            .bearer_auth(token)
            .json(&expired_request)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    drop(a);
    // Closing the caller and reopening the durable owner changes neither identity nor job.
    *store.lock().unwrap() = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
    let reconnected = client(&ca, Some(&desktop));
    let replay = body(
        reconnected
            .post(format!("{endpoint}/v1/shared/jobs"))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["id"], job);
    assert_eq!(replay["start_before_ms"], accepted["start_before_ms"]);
    let status = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/status?project_id=alpha"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status["jobs"][0]["id"], job);
    assert_eq!(status["jobs"][0]["assignee"]["display_name"], "山田");
    assert_eq!(status["jobs"][0]["can_cancel"], true);
    let runner = client(&ca, Some(&worker));
    let environment_url = format!("{endpoint}/v1/shared/environments?project_id=alpha");
    let before_contact = body(
        reconnected
            .get(&environment_url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(before_contact[0]["runner_contact"]["state"], "unconfirmed");
    let assignments_url = format!("{endpoint}/v1/shared/runner/assignments");
    assert_eq!(
        body(
            b.get(format!("{assignments_url}?environment_ids=solver"))
                .send()
                .await
                .unwrap()
        )
        .await,
        json!([])
    );
    assert_eq!(
        body(runner.get(&assignments_url).send().await.unwrap()).await,
        json!([])
    );
    assert_eq!(
        body(
            runner
                .get(format!("{assignments_url}?environment_ids="))
                .send()
                .await
                .unwrap()
        )
        .await,
        json!([])
    );
    assert_eq!(
        runner
            .get(format!("{assignments_url}?environment_ids=solver,solver"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let still_unconfirmed = body(
        reconnected
            .get(&environment_url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        still_unconfirmed[0]["runner_contact"]["state"],
        "unconfirmed"
    );
    let assignment = body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/claim"))
            .json(&json!({"environment_ids":["solver"]}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(assignment["allowed_child_environments"], json!(["solver"]));
    assert_eq!(
        assignment["job"]["start_before_ms"],
        accepted["start_before_ms"]
    );
    let scoped = body(
        runner
            .get(format!(
                "{assignments_url}?environment_ids=solver,deleted-mapping"
            ))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(scoped.as_array().unwrap().len(), 1);
    assert_eq!(scoped[0]["attempt_id"], assignment["attempt_id"]);
    let contact_status = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/status?project_id=alpha"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        contact_status["jobs"][0]["runner_contact"]["state"],
        "recent"
    );
    assert_eq!(
        contact_status["environments"][0]["runner_contact"]["state"],
        "recent"
    );
    let contact_detail = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        contact_detail["runner_contact"],
        contact_status["jobs"][0]["runner_contact"]
    );
    let attempt_url = format!(
        "{endpoint}/v1/shared/runner/attempts/{}",
        assignment["attempt_id"].as_str().unwrap()
    );
    assert_eq!(
        body(runner.get(&attempt_url).send().await.unwrap()).await["state"],
        "assigned"
    );
    assert_eq!(b.get(&attempt_url).send().await.unwrap().status(), 403);
    // Data admission is deliberately shared with Desktop polling. Its 429 is a
    // transient read admission result, not evidence that the assigned work failed.
    let archive_url = format!("{attempt_url}/archive");
    let data_permit = super::data::hold_data_slot().await;
    let busy = runner.get(&archive_url).send().await.unwrap();
    assert_eq!(busy.status(), 429);
    assert_eq!(busy.json::<Value>().await.unwrap()["code"], "capacity");
    drop(data_permit);
    assert!(
        body(runner.get(&archive_url).send().await.unwrap())
            .await
            .is_null()
    );
    assert_eq!(
        body(runner.get(&attempt_url).send().await.unwrap()).await["state"],
        "assigned"
    );
    let assigned_asset_url = format!("{attempt_url}/assets/{}", asset["id"].as_str().unwrap());
    for data_url in [&archive_url, &assigned_asset_url] {
        // A new Hub observation lifetime must still notice authenticated data
        // transfer while the Runner's control tick is waiting for preparation.
        *store.lock().unwrap() = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
        assert_eq!(b.get(data_url).send().await.unwrap().status(), 403);
        let unseen = body(
            reconnected
                .get(format!("{endpoint}/v1/shared/jobs/{job}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(unseen["runner_contact"]["state"], "unconfirmed");
        body(runner.get(data_url).send().await.unwrap()).await;
        let after_data = body(
            reconnected
                .get(format!("{endpoint}/v1/shared/status?project_id=alpha"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            after_data["jobs"][0]["runner_contact"]["state"], "recent",
            "exact Runner data transfer must count as contact"
        );
        assert_eq!(
            after_data["environments"][0]["runner_contact"]["state"],
            "unconfirmed"
        );
        assert_eq!(after_data["environments"][0]["occupied"], 1);
    }
    let assigned_input = body(runner.get(&assigned_asset_url).send().await.unwrap()).await;
    assert_eq!(assigned_input["asset"]["id"], asset["id"]);
    assert_eq!(
        b.get(&assigned_asset_url).send().await.unwrap().status(),
        403
    );
    let started = json!({"event_id":"started","attempt_id":assignment["attempt_id"],"generation":assignment["generation"],"outcome":{"kind":"started"}});
    assert_eq!(
        b.post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&started)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&started)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let approval_report = json!({"event_id":"permission-request","attempt_id":assignment["attempt_id"],"generation":assignment["generation"],"outcome":{"kind":"approval_requested","approval_id":"operation-one","request":{"operation":"controlled fixture write"},"expires_at_ms":crate::service::now_ms()+60000}});
    *store.lock().unwrap() = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
    let content = b"runner response";
    let upload = json!({"generation":assignment["generation"],"kind":"artifact","base_sha256":null,"upload":{"request_id":"contact-artifact","name":"contact.txt","content_base64":STANDARD.encode(content),"sha256":format!("{:x}",Sha256::digest(content))}});
    let upload_url = format!("{attempt_url}/assets");
    assert_eq!(
        b.post(&upload_url)
            .json(&upload)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut wrong_generation = upload.clone();
    wrong_generation["generation"] = json!(assignment["generation"].as_u64().unwrap() + 1);
    assert_eq!(
        runner
            .post(&upload_url)
            .json(&wrong_generation)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let unobserved = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(unobserved["runner_contact"]["state"], "unconfirmed");
    body(runner.post(&upload_url).json(&upload).send().await.unwrap()).await;
    let uploaded = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/status?project_id=alpha"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(uploaded["jobs"][0]["runner_contact"]["state"], "recent");
    assert_eq!(uploaded["jobs"][0]["state"], "running");
    assert_eq!(
        uploaded["environments"][0]["runner_contact"]["state"],
        "unconfirmed"
    );
    body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&approval_report)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let permission = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/jobs/{job}/approval"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(permission["can_decide"], true);
    assert_eq!(
        permission["request"]["operation"],
        "controlled fixture write"
    );
    body(
        reconnected
            .post(format!(
                "{endpoint}/v1/shared/jobs/{job}/approvals/operation-one/decision"
            ))
            .bearer_auth(token)
            .json(&json!({"decision":"approve"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let answer=body(runner.post(format!("{endpoint}/v1/shared/runner/approvals/operation-one/consume")).json(&json!({"attempt_id":assignment["attempt_id"],"generation":assignment["generation"]})).send().await.unwrap()).await;
    assert_eq!(answer["decision"], "approve");
    assert_eq!(answer["approval_id"], "operation-one");
    let uncertain = json!({"event_id":"uncertain","attempt_id":assignment["attempt_id"],"generation":assignment["generation"],"outcome":{"kind":"uncertain","reason":"Runner再起動後の照合"}});
    body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&uncertain)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        body(runner.get(&attempt_url).send().await.unwrap()).await["uncertainty_reason"],
        "Runner再起動後の照合"
    );
    let status = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/status?project_id=alpha"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status["jobs"][0]["uncertainty_reason"],
        "Runner再起動後の照合"
    );
    let done = json!({"event_id":"finished","attempt_id":assignment["attempt_id"],"generation":assignment["generation"],"outcome":{"kind":"finished","success":true,"result":{"answer":42},"resources_released":true}});
    body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&done)
            .send()
            .await
            .unwrap(),
    )
    .await;
    body(
        runner
            .post(format!("{endpoint}/v1/shared/runner/report"))
            .json(&done)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let visible = body(
        reconnected
            .get(format!("{endpoint}/v1/shared/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(visible["state"], "succeeded");
    assert_eq!(visible["result"]["answer"], 42);
    {
        let mut network = network.lock().unwrap();
        let revision = network.revision();
        network.set_admission(&revision, &desktop.id, true).unwrap();
    }
    let before_jobs: u64 = store
        .lock()
        .unwrap()
        .db
        .query_row("SELECT COUNT(*) FROM shared_jobs", [], |r| r.get(0))
        .unwrap();
    let forwarded = json!({"request_id":"stopped-actor","environment_id":"solver","project_id":"alpha","actor_device_id":desktop.id,"principal_session":token,"source":"desktop","title":"must not start"});
    assert_eq!(
        runner
            .post(format!("{endpoint}/v1/shared/runner/local-admissions"))
            .json(&forwarded)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .db
            .query_row("SELECT COUNT(*) FROM shared_jobs", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        before_jobs
    );
    body(
        reconnected
            .post(format!("{endpoint}/v1/shared/logout"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        reconnected
            .get(format!("{endpoint}/v1/shared/jobs/{job}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn revoked_runner_can_settle_its_existing_attempts_but_cannot_start_or_delegate() {
    let dir = tempfile::tempdir().unwrap();
    let mut network =
        DeviceNetwork::open(&dir.path().join("devices.json"), "revoked-shared-test").unwrap();
    let worker = device(&mut network, "Runner E");
    let other = device(&mut network, "Runner D");
    let ca = network.ca_certificate().to_owned();
    let (certificate, key) = network
        .server_certificate("127.0.0.1".parse().unwrap(), crate::service::now_ms())
        .unwrap();
    let mut work = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
    let now = crate::service::now_ms();
    let admin =
        identity::bootstrap_admin(&work.db, "admin", "管理者", "test-password-12345", now).unwrap();
    work.create_project(&admin, "alpha", "Project Alpha")
        .unwrap();
    identity::set_project_role(
        &work.db,
        &admin,
        "alpha",
        &admin.user_id,
        Some(ProjectRole::Contributor),
        now,
    )
    .unwrap();
    work.register_environment(
        &admin,
        &Environment {
            id: "solver".into(),
            label: "Gurobi".into(),
            resource_id: "wine".into(),
            runner_id: worker.id.clone(),
            capacity: 2,
            project_ids: vec!["alpha".into()],
        },
    )
    .unwrap();
    let input = |id: &str| SubmitJob {
        request_id: id.into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: "solver".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "停止確認".into(),
        input: json!({}),
        descendant_budget: 1,
        start_before_ms: None,
    };
    work.submit(&admin, &input("running"), now).unwrap();
    let running = work.claim(&worker.id, now).unwrap().unwrap();
    work.report(
        &worker.id,
        &Report {
            event_id: "started".into(),
            attempt_id: running.attempt_id.clone(),
            generation: running.generation,
            outcome: ReportOutcome::Started,
        },
        now,
    )
    .unwrap();
    work.submit(&admin, &input("prepared"), now).unwrap();
    let prepared = work.claim(&worker.id, now).unwrap().unwrap();
    let store = Arc::new(Mutex::new(work));
    let network = Arc::new(Mutex::new(network));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("https://{address}");
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
        .unwrap()
    });
    let runner = client(&ca, Some(&worker));
    {
        let mut network = network.lock().unwrap();
        let revision = network.revision();
        network
            .set_device(
                &revision,
                &worker.id,
                "Runner E".into(),
                BTreeSet::new(),
                true,
            )
            .unwrap();
    }
    let report_url = format!("{endpoint}/v1/shared/runner/report");
    for (assignment, outcome) in [
        (&prepared, json!({"kind":"started"})),
        (
            &running,
            json!({"kind":"yield_to_child","checkpoint":{},"child_environment_id":"solver", "child_title":"Child", "child_input":{}, "resources_released":true}),
        ),
    ] {
        assert_eq!(
            runner
                .post(&report_url)
                .json(
                    &json!({"event_id":"denied", "attempt_id":assignment.attempt_id,
            "generation":assignment.generation,"outcome":outcome})
                )
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        runner
            .post(format!("{endpoint}/v1/shared/runner/claim"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        runner
            .get(format!("{endpoint}/v1/shared/runner/assignments"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    {
        let mut work = store.lock().unwrap();
        assert_eq!(
            work.job(&admin, &running.job.id).unwrap().state,
            JobState::Running
        );
        assert_eq!(
            work.job(&admin, &prepared.job.id).unwrap().state,
            JobState::Assigned
        );
        assert_eq!(work.list(&admin, "alpha", None, 100).unwrap().len(), 2);
        for job in [&running.job.id, &prepared.job.id] {
            assert_eq!(
                work.cancel(&admin, job, crate::service::now_ms())
                    .unwrap()
                    .state,
                JobState::Cancelling
            );
        }
    }
    let finished = |assignment: &Assignment, released: bool| {
        json!({
            "event_id":"stopped","attempt_id":assignment.attempt_id,"generation":assignment.generation,
            "outcome":{"kind":"finished","success":false,"result":{"stopped":true},"resources_released":released}
        })
    };
    let settled = finished(&running, true);
    assert_eq!(
        client(&ca, None)
            .post(&report_url)
            .json(&settled)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client(&ca, Some(&other))
            .post(&report_url)
            .json(&settled)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut stale = settled.clone();
    stale["generation"] = json!(running.generation + 1);
    assert_eq!(
        runner
            .post(&report_url)
            .json(&stale)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        runner
            .post(&report_url)
            .json(&finished(&running, false))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    for assignment in [&running, &prepared] {
        let response = body(
            runner
                .post(&report_url)
                .json(&finished(assignment, true))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(response["state"], "cancelled");
    }
    assert_eq!(
        body(
            runner
                .post(&report_url)
                .json(&settled)
                .send()
                .await
                .unwrap()
        )
        .await["state"],
        "cancelled"
    );
    assert!(
        store
            .lock()
            .unwrap()
            .assignments(&worker.id)
            .unwrap()
            .is_empty()
    );
    stop.send(()).unwrap();
    server.await.unwrap();
}
