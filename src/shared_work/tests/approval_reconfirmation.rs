use super::*;

fn expired_approval() -> (Fixture, Assignment) {
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
    f.submit("parent", "approval-expiry", 1);
    let attempt = f.work.claim("WinC", 40).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "old-approval");
    (f, attempt)
}

#[test]
fn expired_approval_parks_exact_operation_without_refusal_or_periodic_reissue() {
    let (mut f, attempt) = expired_approval();
    let queued = f.submit("parent", "second-user-work", 0);
    let before = f.work.job(&f.alice, &attempt.job.id).unwrap();
    f.reopen();
    for now in [1000, 2000, 73 * 60 * 60 * 1000] {
        assert!(
            f.work
                .consume_approval(
                    "WinC",
                    "old-approval",
                    &attempt.attempt_id,
                    attempt.generation,
                    now
                )
                .unwrap()
                .is_none()
        );
        let approval = f
            .work
            .approval(&f.alice, &attempt.job.id, now)
            .unwrap()
            .unwrap();
        assert_eq!(approval.id, "old-approval");
        assert_eq!(approval.status, "expired");
        assert!(!approval.can_decide);
        assert!(approval.can_reconfirm);
        assert!(
            f.work
                .decide_approval(
                    &f.alice,
                    &attempt.job.id,
                    "old-approval",
                    ApprovalDecision::Approve,
                    now
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
                    now
                )
                .is_err()
        );
    }
    assert!(
        f.work
            .report(
                "WinC",
                &Report {
                    event_id: "automatic-reissue".into(),
                    attempt_id: attempt.attempt_id.clone(),
                    generation: attempt.generation,
                    outcome: ReportOutcome::ApprovalRequested {
                        approval_id: "unsolicited-new-id".into(),
                        request: json!({}),
                        expires_at_ms: 73 * 60 * 60 * 1000 + 1000
                    },
                },
                73 * 60 * 60 * 1000
            )
            .is_err(),
        "expiry alone must not authorize a new approval ID"
    );
    let current = f.work.job(&f.alice, &attempt.job.id).unwrap();
    assert_eq!(current.state, JobState::Running);
    assert_eq!(current.revision, before.revision);
    assert_eq!(current.assignee_id, f.alice.user_id);
    assert!(!f.work.assignments("WinC").unwrap()[0].stop_requested);
    assert!(f.work.claim("WinC", 73 * 60 * 60 * 1000).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Queued
    );
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_approvals", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn origin_revisit_reissues_once_and_only_fresh_exact_choice_authorizes_effect() {
    for approved_but_not_consumed in [false, true] {
        let (mut f, attempt) = expired_approval();
        if approved_but_not_consumed {
            f.work
                .decide_approval(
                    &f.alice,
                    &attempt.job.id,
                    "old-approval",
                    ApprovalDecision::Approve,
                    999,
                )
                .unwrap();
        }
        assert!(
            f.work
                .consume_approval(
                    "WinC",
                    "old-approval",
                    &attempt.attempt_id,
                    attempt.generation,
                    1000
                )
                .unwrap()
                .is_none()
        );
        assert!(
            f.work
                .reconfirm_expired_approval_for_device(
                    &f.bob,
                    &attempt.job.id,
                    "old-approval",
                    1000,
                    None
                )
                .is_err()
        );
        assert!(
            f.work
                .reconfirm_expired_approval_for_device(
                    &f.alice,
                    &attempt.job.id,
                    "old-approval",
                    999,
                    None
                )
                .is_err()
        );
        for now in [1000, 1001] {
            f.work
                .reconfirm_expired_approval_for_device(
                    &f.alice,
                    &attempt.job.id,
                    "old-approval",
                    now,
                    None,
                )
                .unwrap();
        }
        f.reopen();
        let reply = f
            .work
            .consume_approval(
                "WinC",
                "old-approval",
                &attempt.attempt_id,
                attempt.generation,
                1002,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            json!({"approval_id":"old-approval","reconfirmation_required":true})
        );
        let old = f
            .work
            .approval(&f.alice, &attempt.job.id, 1002)
            .unwrap()
            .unwrap();
        f.work
            .report(
                "WinC",
                &Report {
                    event_id: "replacement".into(),
                    attempt_id: attempt.attempt_id.clone(),
                    generation: attempt.generation,
                    outcome: ReportOutcome::ApprovalRequested {
                        approval_id: "new-approval".into(),
                        request: old.request,
                        expires_at_ms: 2000,
                    },
                },
                1003,
            )
            .unwrap();
        f.work
            .decide_approval(
                &f.alice,
                &attempt.job.id,
                "new-approval",
                ApprovalDecision::Approve,
                1004,
            )
            .unwrap();
        let reply = f
            .work
            .consume_approval(
                "WinC",
                "new-approval",
                &attempt.attempt_id,
                attempt.generation,
                1005,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            json!({"approval_id":"new-approval","decision":"approve"})
        );
        f.work
            .authorize_effect(
                "WinC",
                &attempt.attempt_id,
                attempt.generation,
                attempt.authority_generation,
                Some("new-approval"),
                1005,
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
                    1005
                )
                .is_err()
        );
        assert_eq!(
            f.work.job(&f.alice, &attempt.job.id).unwrap().state,
            JobState::Running
        );
    }
}

#[test]
fn parked_approval_still_requires_exact_request_attempt_runner_and_generation() {
    let (mut f, attempt) = expired_approval();
    for (runner, approval, attempt_id, generation) in [
        (
            "WinE",
            "old-approval",
            attempt.attempt_id.as_str(),
            attempt.generation,
        ),
        (
            "WinC",
            "missing-approval",
            attempt.attempt_id.as_str(),
            attempt.generation,
        ),
        (
            "WinC",
            "old-approval",
            "another-attempt",
            attempt.generation,
        ),
        (
            "WinC",
            "old-approval",
            attempt.attempt_id.as_str(),
            attempt.generation + 1,
        ),
    ] {
        assert!(
            f.work
                .consume_approval(runner, approval, attempt_id, generation, 1000)
                .is_err()
        );
    }
    assert_eq!(
        f.work.attempt("WinC", &attempt.attempt_id).unwrap().state,
        "running"
    );
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
        let (mut f, attempt) = expired_approval();
        f.work
            .reconfirm_expired_approval_for_device(
                &f.alice,
                &attempt.job.id,
                "old-approval",
                1000,
                None,
            )
            .unwrap();
        match change {
            "cancel" => {
                f.work.cancel(&f.alice, &attempt.job.id, 1001).unwrap();
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
                        1001,
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
                        1001,
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
                    1001,
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
                f.work.manage(&f.admin, &command, 1001).unwrap();
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
                    1002
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
                    1002
                )
                .is_err(),
            "{change} must not authorize the old approval"
        );
    }
}
#[test]
fn expired_unconsumed_child_permission_remains_visible_from_parent_for_reconfirmation() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "parent", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&child, "start", ReportOutcome::Started);
    request_approval(&mut f, &child, "child-approval");
    f.work
        .decide_approval(
            &f.alice,
            &child.job.id,
            "child-approval",
            ApprovalDecision::Approve,
            999,
        )
        .unwrap();
    assert!(
        f.work
            .approval_exact_for_device(&f.alice, &root.id, 1000, None)
            .unwrap()
            .is_none()
    );
    let view = f
        .work
        .approval_for_device(&f.alice, &root.id, 1000, None)
        .unwrap()
        .unwrap();
    assert_eq!(view.id, "child-approval");
    assert_eq!(view.context.job_id, child.job.id);
    assert!(view.can_reconfirm);
    assert!(!view.can_decide);
}
