use super::*;

fn pending_handover() -> (Fixture, Assignment) {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        30,
    )
    .unwrap();
    let root = f.submit("parent", "approval-handover", 1);
    let attempt = f.work.claim("WinC", 40).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "old-approval");
    let pending = f
        .work
        .consume_approval(
            "WinC",
            "old-approval",
            &attempt.attempt_id,
            attempt.generation,
            40,
        )
        .unwrap();
    assert!(pending.is_none());
    assert_eq!(serde_json::to_value(pending).unwrap(), Value::Null);
    let current = f.work.job(&f.alice, &root.id).unwrap();
    f.work
        .handover(
            &f.alice,
            &root.id,
            &HandoverRequest {
                expected_revision: current.revision,
                new_assignee_id: f.bob.user_id.clone(),
            },
            41,
        )
        .unwrap();
    (f, attempt)
}

#[test]
fn invalidated_approval_reconfirms_without_settling_handover_or_releasing_capacity() {
    let (mut f, attempt) = pending_handover();
    let queued = f.submit("parent", "second-user-work", 0);
    let before = f.work.job(&f.alice, &attempt.job.id).unwrap();
    f.reopen();
    for now in [42, 43] {
        let reply = f
            .work
            .consume_approval(
                "WinC",
                "old-approval",
                &attempt.attempt_id,
                attempt.generation,
                now,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            json!({"approval_id":"old-approval","reconfirmation_required":true})
        );
    }
    let current = f.work.job(&f.alice, &attempt.job.id).unwrap();
    assert_eq!(current.state, JobState::Running);
    assert_eq!(current.assignee_id, f.alice.user_id);
    assert_eq!(current.authority_generation, attempt.authority_generation);
    assert_eq!(current.revision, before.revision);
    assert!(
        f.work
            .handover_view(&f.alice, &current.id)
            .unwrap()
            .pending
            .is_some()
    );
    assert!(f.work.claim("WinC", 43).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Queued
    );
    let old = f.work.approval(&f.alice, &current.id, 43).unwrap().unwrap();
    assert_eq!(old.status, "cancelled");
    assert!(!old.can_decide);
    assert!(
        f.work
            .decide_approval(
                &f.alice,
                &current.id,
                "old-approval",
                ApprovalDecision::Approve,
                43
            )
            .is_err()
    );
    assert!(
        f.work
            .authorize_effect(
                "WinC",
                &attempt.attempt_id,
                attempt.generation,
                attempt.authority_generation,
                Some("old-approval"),
                43
            )
            .is_err()
    );

    f.work
        .report(
            "WinC",
            &Report {
                event_id: "new-request".into(),
                attempt_id: attempt.attempt_id.clone(),
                generation: attempt.generation,
                outcome: ReportOutcome::ApprovalRequested {
                    approval_id: "new-approval".into(),
                    request: old.request,
                    expires_at_ms: 1000,
                },
            },
            44,
        )
        .unwrap();
    assert!(
        f.work
            .decide_approval(
                &f.bob,
                &current.id,
                "new-approval",
                ApprovalDecision::Approve,
                45
            )
            .is_err()
    );
    f.work
        .decide_approval(
            &f.alice,
            &current.id,
            "new-approval",
            ApprovalDecision::Approve,
            45,
        )
        .unwrap();
    let answer = f
        .work
        .consume_approval(
            "WinC",
            "new-approval",
            &attempt.attempt_id,
            attempt.generation,
            46,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        json!({"approval_id":"new-approval","decision":"approve"})
    );
    f.work
        .authorize_effect(
            "WinC",
            &attempt.attempt_id,
            attempt.generation,
            attempt.authority_generation,
            Some("new-approval"),
            46,
        )
        .unwrap();
    assert!(
        f.work
            .authorize_effect(
                "WinC",
                &attempt.attempt_id,
                attempt.generation,
                attempt.authority_generation,
                Some("old-approval"),
                46
            )
            .is_err()
    );

    f.work
        .report(
            "WinC",
            &Report {
                event_id: "safe-yield".into(),
                attempt_id: attempt.attempt_id.clone(),
                generation: attempt.generation,
                outcome: child(),
            },
            47,
        )
        .unwrap();
    let transferred = f.work.job(&f.bob, &current.id).unwrap();
    assert_eq!(transferred.assignee_id, f.bob.user_id);
    assert_eq!(
        transferred.authority_generation,
        attempt.authority_generation + 1
    );
    assert_eq!(transferred.state, JobState::WaitingChild);
    assert!(
        f.work
            .handover_view(&f.bob, &current.id)
            .unwrap()
            .pending
            .is_none()
    );
    let child = f.work.claim("WinE", 48).unwrap().unwrap();
    assert_eq!(child.job.assignee_id, f.bob.user_id);
    assert_eq!(child.authority_generation, transferred.authority_generation);
    assert!(
        f.work
            .consume_approval(
                "WinC",
                "old-approval",
                &attempt.attempt_id,
                attempt.generation,
                48
            )
            .is_err()
    );
    assert!(
        f.work
            .authorize_effect(
                "WinC",
                &attempt.attempt_id,
                attempt.generation,
                attempt.authority_generation,
                Some("new-approval"),
                48
            )
            .is_err()
    );
}

#[test]
fn reconfirmation_requires_exact_request_attempt_runner_generation_and_live_deadline() {
    let (mut f, attempt) = pending_handover();
    for (runner, approval, attempt_id, generation, now) in [
        (
            "WinE",
            "old-approval",
            attempt.attempt_id.as_str(),
            attempt.generation,
            42,
        ),
        (
            "WinC",
            "missing-approval",
            attempt.attempt_id.as_str(),
            attempt.generation,
            42,
        ),
        (
            "WinC",
            "old-approval",
            "another-attempt",
            attempt.generation,
            42,
        ),
        (
            "WinC",
            "old-approval",
            attempt.attempt_id.as_str(),
            attempt.generation + 1,
            42,
        ),
        (
            "WinC",
            "old-approval",
            attempt.attempt_id.as_str(),
            attempt.generation,
            1000,
        ),
    ] {
        assert!(
            f.work
                .consume_approval(runner, approval, attempt_id, generation, now)
                .is_err()
        );
    }
    assert_eq!(
        f.work.attempt("WinC", &attempt.attempt_id).unwrap().state,
        "running"
    );
    assert!(f.work.claim("WinC", 1000).unwrap().is_none());
}

#[test]
fn reconfirmation_rejects_cancellation_uncertainty_terminal_and_revoked_authority() {
    for change in [
        "cancel",
        "uncertain",
        "terminal",
        "role",
        "archive",
        "environment",
    ] {
        let (mut f, attempt) = pending_handover();
        match change {
            "cancel" => {
                f.work.cancel(&f.alice, &attempt.job.id, 42).unwrap();
            }
            "uncertain" => {
                f.work
                    .report(
                        "WinC",
                        &Report {
                            event_id: "uncertain".into(),
                            attempt_id: attempt.attempt_id.clone(),
                            generation: attempt.generation,
                            outcome: ReportOutcome::Uncertain {
                                reason: "Worker state unavailable".into(),
                            },
                        },
                        42,
                    )
                    .unwrap();
            }
            "terminal" => {
                f.work
                    .report(
                        "WinC",
                        &Report {
                            event_id: "finished".into(),
                            attempt_id: attempt.attempt_id.clone(),
                            generation: attempt.generation,
                            outcome: finished(),
                        },
                        42,
                    )
                    .unwrap();
            }
            "role" => {
                identity::set_project_role(
                    &f.work.db,
                    &f.admin,
                    "alpha",
                    &f.alice.user_id,
                    Some(ProjectRole::Reader),
                    42,
                )
                .unwrap();
            }
            "archive" | "environment" => {
                let request = if change == "archive" {
                    ManagementRequest::UpdateProject {
                        id: "alpha".into(),
                        label: "alpha".into(),
                        archived: true,
                    }
                } else {
                    ManagementRequest::UpdateEnvironment {
                        id: "parent".into(),
                        label: "進行役".into(),
                        resource_id: "parent-slot".into(),
                        runner_id: "WinC".into(),
                        enabled: false,
                        project_ids: vec!["alpha".into(), "beta".into()],
                    }
                };
                let command = ManagementCommand {
                    request_id: format!("disable-{change}"),
                    expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
                        .as_str()
                        .unwrap()
                        .into(),
                    request,
                };
                f.work.manage(&f.admin, &command, 42).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            f.work
                .consume_approval(
                    "WinC",
                    "old-approval",
                    &attempt.attempt_id,
                    attempt.generation,
                    43
                )
                .is_err(),
            "{change} must not reissue approval"
        );
        assert!(
            f.work
                .authorize_effect(
                    "WinC",
                    &attempt.attempt_id,
                    attempt.generation,
                    attempt.authority_generation,
                    Some("old-approval"),
                    43
                )
                .is_err(),
            "{change} must not authorize the old approval"
        );
    }
}
