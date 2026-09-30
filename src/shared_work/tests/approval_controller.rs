use super::*;
use rusqlite::params;

fn submit(f: &mut Fixture, environment: &str) -> Job {
    f.work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "controller-turn".into(),
                project_id: "alpha".into(),
                project_participation: Some(1),
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: environment.into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "Build across PCs".into(),
                input: json!({"prompt":"Use WinA and WinB"}),
                descendant_budget: 2,
                start_before_ms: None,
            },
            10,
            Some("WinA"),
        )
        .unwrap()
}

#[test]
fn pending_child_is_visible_on_original_conversation_and_only_origin_pc_may_decide() {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Manager),
        1,
    )
    .unwrap();
    let root = submit(&mut f, "parent");
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&parent, "parent-start", ReportOutcome::Started);
    request_approval(&mut f, &parent, "parent-approval");
    f.work
        .decide_approval_for_device(
            &f.alice,
            &root.id,
            "parent-approval",
            ApprovalDecision::Approve,
            21,
            Some("WinA"),
        )
        .unwrap();
    f.work
        .consume_approval(
            "WinC",
            "parent-approval",
            &parent.attempt_id,
            parent.generation,
            22,
        )
        .unwrap();
    f.report(&parent, "delegate", child());
    let child = f.work.claim("WinE", 23).unwrap().unwrap();
    f.report(&child, "child-start", ReportOutcome::Started);
    request_approval(&mut f, &child, "child-approval");

    let view = f
        .work
        .approval_for_device(&f.alice, &root.id, 24, Some("WinA"))
        .unwrap()
        .unwrap();
    assert_eq!(view.id, "child-approval");
    assert_eq!(view.status, "pending");
    assert!(view.can_decide);
    assert_eq!(view.context.job_id, child.job.id);
    assert_eq!(view.context.root_id, root.id);
    assert_eq!(view.context.conversation_id, root.conversation_id);
    assert_eq!(view.context.controller_device_id.as_deref(), Some("WinA"));
    assert_eq!(view.context.execution_device_id, "WinE");
    let legacy = f
        .work
        .approval_exact_for_device(&f.alice, &root.id, 24, Some("WinA"))
        .unwrap()
        .unwrap();
    assert_eq!(legacy.id, "parent-approval");
    assert_eq!(legacy.status, "consumed");
    let inbox = f
        .work
        .inbox_for_device(&f.alice, None, 10, 24, Some("WinA"))
        .unwrap();
    let notification = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["approval_id"] == "child-approval")
        .unwrap();
    assert_eq!(notification["can_act"], true);
    assert_eq!(notification["approval_context"]["job_id"], child.job.id);
    for (actor, device) in [
        (&f.alice, Some("WinE")),
        (&f.alice, Some("WinC")),
        (&f.alice, None),
        (&f.bob, Some("WinA")),
    ] {
        assert!(
            !f.work
                .approval_for_device(actor, &root.id, 24, device)
                .unwrap()
                .unwrap()
                .can_decide
        );
        assert!(matches!(
            f.work.decide_approval_for_device(
                actor,
                &child.job.id,
                "child-approval",
                ApprovalDecision::Approve,
                24,
                device
            ),
            Err(WorkError::Forbidden)
        ));
    }
    assert!(matches!(
        f.work.decide_approval_for_device(
            &f.alice,
            &root.id,
            "child-approval",
            ApprovalDecision::Approve,
            24,
            Some("WinA")
        ),
        Err(WorkError::Conflict)
    ));
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "child-approval",
                &child.attempt_id,
                child.generation,
                24
            )
            .unwrap()
            .is_none()
    );
    f.reopen();
    assert!(
        f.work
            .approval_for_device(&f.alice, &root.id, 25, Some("WinA"))
            .unwrap()
            .unwrap()
            .can_decide
    );
    f.work
        .decide_approval_for_device(
            &f.alice,
            &child.job.id,
            "child-approval",
            ApprovalDecision::Approve,
            26,
            Some("WinA"),
        )
        .unwrap();
    for now in [27, 28] {
        assert!(matches!(
            f.work
                .consume_approval(
                    "WinE",
                    "child-approval",
                    &child.attempt_id,
                    child.generation,
                    now
                )
                .unwrap(),
            Some(ApprovalConsumeResult::Answer(ApprovalAnswer {
                decision: ApprovalDecision::Approve,
                ..
            }))
        ));
    }
    assert!(
        f.work
            .authorize_effect(
                "WinE",
                &child.attempt_id,
                child.generation,
                child.authority_generation,
                Some("child-approval"),
                29
            )
            .is_ok()
    );
    f.work.cancel(&f.alice, &root.id, 30).unwrap();
    assert!(
        f.work
            .authorize_effect(
                "WinE",
                &child.attempt_id,
                child.generation,
                child.authority_generation,
                Some("child-approval"),
                31
            )
            .is_err()
    );
}

#[test]
fn rejoined_or_removed_controller_cannot_consume_an_earlier_approval() {
    for change in ["rejoin", "remove"] {
        let mut f = Fixture::new();
        let job = submit(&mut f, "solver");
        let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
        f.report(&attempt, "start", ReportOutcome::Started);
        request_approval(&mut f, &attempt, "approval");
        // Model current participation explicitly, while keeping its execution
        // grant unchanged. Only the controller's participation is under test.
        f.work
            .db
            .execute(
                "UPDATE shared_projects SET legacy_devices_unrestricted=0 WHERE id='alpha'",
                [],
            )
            .unwrap();
        f.work.db.execute("INSERT INTO shared_project_devices(project_id,device_id,can_control,can_execute,participation_generation,environment_id) VALUES('alpha','WinA',1,0,1,NULL),('alpha','WinE',0,1,1,'solver')", []).unwrap();
        f.work
            .decide_approval_for_device(
                &f.alice,
                &job.id,
                "approval",
                ApprovalDecision::Approve,
                21,
                Some("WinA"),
            )
            .unwrap();
        match change {
            "rejoin" => f.work.db.execute("UPDATE shared_project_devices SET participation_generation=2 WHERE device_id='WinA'", []).unwrap(),
            _ => f.work.db.execute("UPDATE shared_project_devices SET can_control=0 WHERE device_id='WinA'", []).unwrap(),
        };
        assert!(
            f.work
                .consume_approval(
                    "WinE",
                    "approval",
                    &attempt.attempt_id,
                    attempt.generation,
                    22
                )
                .is_err()
        );
        let consumed: Option<u64> = f
            .work
            .db
            .query_row(
                "SELECT consumed_at_ms FROM shared_approvals WHERE id=?1",
                params!["approval"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(consumed, None);
        assert!(
            !f.work
                .approval_for_device(&f.alice, &job.id, 22, Some("WinA"))
                .unwrap()
                .unwrap()
                .can_decide
        );
    }
}

#[test]
fn origin_human_deny_or_stop_is_delivered_exactly_and_never_authorizes_effect() {
    for decision in [ApprovalDecision::Deny, ApprovalDecision::Stop] {
        let mut f = Fixture::new();
        let job = submit(&mut f, "solver");
        let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
        f.report(&attempt, "start", ReportOutcome::Started);
        request_approval(&mut f, &attempt, "approval");
        f.work
            .decide_approval_for_device(&f.alice, &job.id, "approval", decision, 21, Some("WinA"))
            .unwrap();
        let Some(ApprovalConsumeResult::Answer(answer)) = f
            .work
            .consume_approval(
                "WinE",
                "approval",
                &attempt.attempt_id,
                attempt.generation,
                22,
            )
            .unwrap()
        else {
            panic!("exact answer expected")
        };
        assert_eq!(answer.decision, decision);
        assert!(
            f.work
                .authorize_effect(
                    "WinE",
                    &attempt.attempt_id,
                    attempt.generation,
                    attempt.authority_generation,
                    Some("approval"),
                    23
                )
                .is_err()
        );
        assert!(
            f.work
                .decide_approval_for_device(
                    &f.alice,
                    &job.id,
                    "approval",
                    ApprovalDecision::Approve,
                    24,
                    Some("WinA")
                )
                .is_err()
        );
    }
}

#[test]
fn child_human_refusal_does_not_resume_the_parent_after_failed_report() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    for decision in [ApprovalDecision::Deny, ApprovalDecision::Stop] {
        let mut f = Fixture::new();
        let root = submit(&mut f, "parent");
        let parent = f.work.claim("WinC", 11).unwrap().unwrap();
        f.report(&parent, "parent-start", ReportOutcome::Started);
        let archive = serde_json::to_vec(&json!({"version":1,"job_id":root.id,
            "project_id":"alpha","environment_id":"parent",
            "transcript":[{"kind":"message","payload":{"text":"Delegating the requested work"}}]}))
        .unwrap();
        f.work
            .runner_upload_asset(
                "WinC",
                &parent.attempt_id,
                &RunnerAssetUpload {
                    generation: parent.generation,
                    kind: "archive".into(),
                    base_sha256: None,
                    upload: AssetUpload {
                        request_id: "parent-transcript".into(),
                        name: "history.json".into(),
                        content_base64: STANDARD.encode(&archive),
                        sha256: format!("{:x}", Sha256::digest(&archive)),
                    },
                },
                20,
            )
            .unwrap();
        f.report(&parent, "delegate", child());
        let child = f.work.claim("WinE", 21).unwrap().unwrap();
        f.report(&child, "child-start", ReportOutcome::Started);
        f.report(
            &child,
            "retain-preview",
            ReportOutcome::RetainService {
                service_id: "child-preview".into(),
                expires_at_ms: Some(1000),
            },
        );
        request_approval(&mut f, &child, "child-refusal");
        let decided = f
            .work
            .decide_approval_for_device(
                &f.alice,
                &child.job.id,
                "child-refusal",
                decision,
                22,
                Some("WinA"),
            )
            .unwrap();
        assert_eq!(decided.status, "decided");
        assert_eq!(decided.decision, Some(decision));
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().state,
            JobState::Cancelling
        );
        assert!(
            f.work
                .attempt("WinE", &child.attempt_id)
                .unwrap()
                .assignment
                .stop_requested
        );
        assert!(f.work.runner_services("WinE", 22).unwrap()[0].stop_requested);
        assert!(f.work.claim("WinC", 22).unwrap().is_none());
        assert!(
            f.work
                .authorize_effect(
                    "WinE",
                    &child.attempt_id,
                    child.generation,
                    child.authority_generation,
                    None,
                    22
                )
                .is_err()
        );
        for (event, outcome) in [
            ("new-child", super::child()),
            (
                "new-service",
                ReportOutcome::RetainService {
                    service_id: "bypass".into(),
                    expires_at_ms: Some(1000),
                },
            ),
        ] {
            assert!(
                f.work
                    .report(
                        "WinE",
                        &Report {
                            event_id: event.into(),
                            attempt_id: child.attempt_id.clone(),
                            generation: child.generation,
                            outcome
                        },
                        22
                    )
                    .is_err()
            );
        }
        let revision = f.work.job(&f.alice, &root.id).unwrap().revision;
        f.work
            .decide_approval_for_device(
                &f.alice,
                &child.job.id,
                "child-refusal",
                decision,
                22,
                Some("WinA"),
            )
            .unwrap();
        assert_eq!(f.work.job(&f.alice, &root.id).unwrap().revision, revision);
        let answer = f
            .work
            .consume_approval(
                "WinE",
                "child-refusal",
                &child.attempt_id,
                child.generation,
                23,
            )
            .unwrap()
            .unwrap();
        assert!(
            matches!(answer, ApprovalConsumeResult::Answer(answer) if answer.decision == decision)
        );
        assert!(
            matches!(f.work.consume_approval("WinE", "child-refusal", &child.attempt_id, child.generation, 23).unwrap(),
            Some(ApprovalConsumeResult::Answer(answer)) if answer.decision == decision)
        );
        f.report(
            &child,
            "denied-finish",
            ReportOutcome::Finished {
                success: false,
                result: json!({"message":"The user declined this operation"}),
                resources_released: true,
            },
        );
        let reclaimed = f.work.claim("WinC", 24).unwrap();
        assert!(
            reclaimed.is_none(),
            "human {decision:?} unexpectedly resumed parent {:?}",
            reclaimed.map(|assignment| assignment.job.id)
        );
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().state,
            JobState::Cancelled
        );
        let shown = f
            .work
            .approval_for_device(&f.alice, &root.id, 24, Some("WinA"))
            .unwrap()
            .unwrap();
        assert_eq!(shown.id, "child-refusal");
        assert_eq!(shown.status, "consumed");
        assert_eq!(shown.decision, Some(decision));
        f.reopen();
        f.report(
            &child,
            "preview-stopped",
            ReportOutcome::ServiceStopped {
                service_id: "child-preview".into(),
            },
        );
        let revision = f.work.job(&f.alice, &root.id).unwrap().revision;
        let next = f
            .work
            .continue_job_for_device(
                &f.alice,
                Some("WinA"),
                &root.id,
                &ContinueJob {
                    request_id: "explicit-next-turn".into(),
                    expected_revision: revision,
                    project_participation: Some(1),
                    conversation_epoch: None,
                    prompt: "Now perform the different operation I requested".into(),
                    input_refs: vec![],
                    start_before_ms: None,
                },
                25,
            )
            .unwrap();
        f.work
            .decide_approval_for_device(
                &f.alice,
                &child.job.id,
                "child-refusal",
                decision,
                26,
                Some("WinA"),
            )
            .unwrap();
        assert!(
            f.work
                .decide_approval_for_device(
                    &f.alice,
                    &child.job.id,
                    "child-refusal",
                    ApprovalDecision::Approve,
                    26,
                    Some("WinA")
                )
                .is_err()
        );
        assert!(
            f.work
                .decide_approval_for_device(
                    &f.alice,
                    &child.job.id,
                    "child-refusal",
                    decision,
                    26,
                    Some("WinE")
                )
                .is_err()
        );
        assert_eq!(f.work.claim("WinC", 27).unwrap().unwrap().job.id, next.id);
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().state,
            JobState::Cancelled
        );
    }
}

#[test]
fn retired_handover_keeps_origin_approval_and_does_not_grant_manager_or_executor() {
    let mut f = Fixture::new();
    for actor in [&f.bob, &f.admin] {
        identity::set_project_role(
            &f.work.db,
            &f.admin,
            "alpha",
            &actor.user_id,
            Some(ProjectRole::Manager),
            1,
        )
        .unwrap();
    }
    let root = submit(&mut f, "parent");
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    request_approval(&mut f, &parent, "origin-approval");
    assert!(matches!(
        f.work.handover(
            &f.alice,
            &root.id,
            &HandoverRequest {
                expected_revision: root.revision,
                new_assignee_id: f.bob.user_id.clone()
            },
            21
        ),
        Err(WorkError::Retired)
    ));
    for (actor, device) in [
        (&f.alice, "WinD"),
        (&f.bob, "WinA"),
        (&f.admin, "WinA"),
        (&f.alice, "WinC"),
    ] {
        assert!(
            !f.work
                .approval_for_device(actor, &root.id, 22, Some(device))
                .unwrap()
                .unwrap()
                .can_decide
        );
        assert!(
            f.work
                .decide_approval_for_device(
                    actor,
                    &root.id,
                    "origin-approval",
                    ApprovalDecision::Approve,
                    22,
                    Some(device)
                )
                .is_err()
        );
    }
    assert!(
        f.work
            .approval_for_device(&f.alice, &root.id, 22, Some("WinA"))
            .unwrap()
            .unwrap()
            .can_decide
    );
    f.work
        .decide_approval_for_device(
            &f.alice,
            &root.id,
            "origin-approval",
            ApprovalDecision::Approve,
            23,
            Some("WinA"),
        )
        .unwrap();
}
