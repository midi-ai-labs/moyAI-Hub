use super::*;
use crate::identity::{self, Principal, ProjectRole};
use serde_json::json;

struct Fixture {
    dir: tempfile::TempDir,
    work: SharedWork,
    admin: Principal,
    alice: Principal,
    bob: Principal,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut work = SharedWork::open(&dir.path().join("shared.sqlite3")).unwrap();
        let admin =
            identity::bootstrap_admin(&work.db, "admin", "管理者", "test-password-12345", 1)
                .unwrap();
        let alice = identity::create_user(
            &work.db,
            &admin,
            "alice",
            "山田",
            "test-password-12345",
            false,
            1,
        )
        .unwrap();
        let bob = identity::create_user(
            &work.db,
            &admin,
            "bob",
            "佐藤",
            "test-password-12345",
            false,
            1,
        )
        .unwrap();
        for project in ["alpha", "beta"] {
            work.create_project(&admin, project, project).unwrap();
        }
        identity::set_project_role(
            &work.db,
            &admin,
            "alpha",
            &alice.user_id,
            Some(ProjectRole::Contributor),
            1,
        )
        .unwrap();
        identity::set_project_role(
            &work.db,
            &admin,
            "beta",
            &bob.user_id,
            Some(ProjectRole::Contributor),
            1,
        )
        .unwrap();
        work.register_environment(
            &admin,
            &Environment {
                id: "solver".into(),
                label: "WinE Gurobi".into(),
                resource_id: "wine-slot".into(),
                runner_id: "WinE".into(),
                capacity: 1,
                project_ids: vec!["alpha".into(), "beta".into()],
            },
        )
        .unwrap();
        work.register_environment(
            &admin,
            &Environment {
                id: "parent".into(),
                label: "進行役".into(),
                resource_id: "parent-slot".into(),
                runner_id: "WinC".into(),
                capacity: 1,
                project_ids: vec!["alpha".into(), "beta".into()],
            },
        )
        .unwrap();
        Self {
            dir,
            work,
            admin,
            alice,
            bob,
        }
    }
    fn submit(&mut self, environment: &str, key: &str, budget: u32) -> Job {
        self.work
            .submit(
                &self.alice,
                &SubmitJob {
                    request_id: key.into(),
                    project_id: "alpha".into(),
                    environment_id: environment.into(),
                    title: key.into(),
                    input: json!({"prompt":"test"}),
                    descendant_budget: budget,
                    start_before_ms: None,
                },
                10,
            )
            .unwrap()
    }
    fn report(&mut self, assignment: &Assignment, event: &str, outcome: ReportOutcome) -> Job {
        self.work
            .report(
                &assignment.runner_id,
                &Report {
                    event_id: event.into(),
                    attempt_id: assignment.attempt_id.clone(),
                    generation: assignment.generation,
                    outcome,
                },
                20,
            )
            .unwrap()
    }
    fn reopen(&mut self) {
        self.work = SharedWork::open(&self.dir.path().join("shared.sqlite3")).unwrap();
    }
    fn remove_post_v10_fixture_schema(&self) {
        // Old-format fixtures keep their jobs and identities, but cannot retain
        // later migrations while moving the schema version marker backwards.
        self.work
            .db
            .execute_batch(
                "
            DROP VIEW identity_effective_project_memberships;
            DROP VIEW identity_explicit_project_memberships;
            DROP TRIGGER identity_controller_insert;
            DROP TRIGGER identity_controller_update;
            DROP TRIGGER identity_controller_delete;
            DROP TABLE identity_web_session_devices;
            DROP TABLE identity_local_admin;
            DROP TABLE identity_device_principals;
            ALTER TABLE identity_sessions DROP COLUMN device_binding_generation;
            ALTER TABLE identity_users DROP COLUMN credential_kind;
            DROP TABLE shared_project_devices;
            ALTER TABLE shared_projects DROP COLUMN legacy_devices_unrestricted;
            DROP TABLE identity_setup_codes;
            ALTER TABLE identity_users DROP COLUMN setup_pending;
            DROP TRIGGER identity_refresh_credentials_changed;
            DROP INDEX identity_sessions_refresh;
            ALTER TABLE identity_sessions DROP COLUMN refresh_hash;
            DROP TABLE identity_refresh_sessions;
            -- Released V5 membership view, before device-derived grants existed.
            CREATE VIEW identity_effective_project_memberships AS
             SELECT user_id,project_id,CASE MAX(rank) WHEN 3 THEN 'manager' WHEN 2 THEN 'contributor' ELSE 'reader' END AS role FROM (
             SELECT m.user_id,m.project_id,CASE m.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END AS rank FROM identity_project_memberships m JOIN identity_users u ON u.id=m.user_id WHERE u.disabled=0
             UNION ALL SELECT m.user_id,p.project_id,CASE p.role WHEN 'manager' THEN 3 WHEN 'contributor' THEN 2 ELSE 1 END FROM identity_group_memberships m JOIN identity_groups g ON g.id=m.group_id JOIN identity_group_project_roles p ON p.group_id=g.id JOIN identity_users u ON u.id=m.user_id WHERE g.disabled=0 AND u.disabled=0
             ) GROUP BY user_id,project_id;
        ",
            )
            .unwrap();
    }
}
fn finished() -> ReportOutcome {
    ReportOutcome::Finished {
        success: true,
        result: json!({"answer":42}),
        resources_released: true,
    }
}

#[test]
fn status_explains_shared_capacity_without_disclosing_other_project_work() {
    let mut f = Fixture::new();
    let private = f
        .work
        .submit(
            &f.bob,
            &SubmitJob {
                request_id: "beta-secret".into(),
                project_id: "beta".into(),
                environment_id: "solver".into(),
                title: "秘密の計算依頼".into(),
                input: json!({"secret":"not-visible"}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            2,
        )
        .unwrap();
    let mine = f.submit("solver", "waiting-alpha", 0);
    assert_eq!(f.work.claim("WinE", 3).unwrap().unwrap().job.id, private.id);
    let status = f.work.status(&f.alice, "alpha", None, 50, None).unwrap();
    let environment = status
        .environments
        .iter()
        .find(|e| e.id == "solver")
        .unwrap();
    assert_eq!((environment.occupied, environment.other_occupants), (1, 1));
    assert!(environment.occupants.is_empty());
    assert_eq!(status.jobs[0].id, mine.id);
    assert!(status.jobs[0].can_cancel);
    assert!(
        status.jobs[0]
            .wait_reason
            .as_ref()
            .unwrap()
            .contains("共通利用枠")
    );
    let rendered = serde_json::to_string(&status).unwrap();
    assert!(!rendered.contains(&private.id));
    assert!(!rendered.contains("秘密の計算依頼"));
    assert!(!rendered.contains(&f.bob.user_id));
    assert!(f.work.status(&f.admin, "alpha", None, 50, None).is_err());
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "beta",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        4,
    )
    .unwrap();
    let status = f.work.status(&f.alice, "alpha", None, 50, None).unwrap();
    let environment = status
        .environments
        .iter()
        .find(|e| e.id == "solver")
        .unwrap();
    assert_eq!(environment.other_occupants, 0);
    assert_eq!(environment.occupants[0].job_id, private.id);
    assert_eq!(
        environment.occupants[0].user.display_name,
        f.bob.display_name
    );
    identity::set_project_role(&f.work.db, &f.admin, "beta", &f.alice.user_id, None, 5).unwrap();
    assert!(
        f.work
            .status(&f.alice, "alpha", None, 50, None)
            .unwrap()
            .environments
            .iter()
            .find(|e| e.id == "solver")
            .unwrap()
            .occupants
            .is_empty()
    );
}

#[test]
fn status_pages_are_bounded_and_reader_controls_are_not_administrator_controls() {
    let mut f = Fixture::new();
    for n in 0..23 {
        f.work
            .register_environment(
                &f.admin,
                &Environment {
                    id: format!("extra-{n:02}"),
                    label: format!("環境{n}"),
                    resource_id: "wine-slot".into(),
                    runner_id: "WinE".into(),
                    capacity: 1,
                    project_ids: vec!["alpha".into()],
                },
            )
            .unwrap();
    }
    let first = f.submit("solver", "first", 0);
    let last = f.submit("solver", "last", 0);
    let status = f.work.status(&f.alice, "alpha", None, 1, None).unwrap();
    assert_eq!(status.jobs.len(), 1);
    assert_eq!(status.jobs[0].id, last.id);
    assert_eq!(status.environments.len(), 20);
    let next = f
        .work
        .status(
            &f.alice,
            "alpha",
            status.next_before.as_deref(),
            1,
            status.next_environment_before.as_deref(),
        )
        .unwrap();
    assert_eq!(next.jobs[0].id, first.id);
    assert!(next.next_before.is_none());
    assert_eq!(next.environments.len(), 5);
    assert!(next.next_environment_before.is_none());
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        20,
    )
    .unwrap();
    let status = f.work.status(&f.alice, "alpha", None, 50, None).unwrap();
    assert!(status.jobs.iter().all(|j| !j.can_cancel));
    assert!(f.work.status(&f.alice, "alpha", None, 101, None).is_err());
}

#[test]
fn uncertain_attempt_keeps_capacity_and_cannot_start_or_delegate_after_reopen() {
    let mut f = Fixture::new();
    f.submit("solver", "uncertain", 1);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let report = Report {
        attempt_id: attempt.attempt_id.clone(),
        generation: attempt.generation,
        event_id: "unknown".into(),
        outcome: ReportOutcome::Uncertain {
            reason: "実行中にRunnerが再起動しました".into(),
        },
    };
    f.work.report("WinE", &report, 12).unwrap();
    f.reopen();
    f.work.report("WinE", &report, 13).unwrap();
    let observation = f.work.attempt("WinE", &attempt.attempt_id).unwrap();
    assert_eq!(observation.state, "running");
    assert!(observation.uncertainty_reason.is_some());
    assert!(f.work.attempt("WinC", &attempt.attempt_id).is_err());
    let status = f.work.status(&f.alice, "alpha", None, 50, None).unwrap();
    assert!(status.jobs[0].uncertainty_reason.is_some());
    assert_eq!(
        status
            .environments
            .iter()
            .find(|e| e.id == "solver")
            .unwrap()
            .occupied,
        1
    );
    assert!(
        f.work
            .report(
                "WinE",
                &Report {
                    event_id: "retry-yield".into(),
                    outcome: child(),
                    ..report.clone()
                },
                14
            )
            .is_err()
    );
    f.work.cancel(&f.alice, &attempt.job.id, 15).unwrap();
    assert!(
        f.work
            .attempt("WinE", &attempt.attempt_id)
            .unwrap()
            .assignment
            .stop_requested
    );
    f.submit("solver", "after", 0);
    assert_eq!(
        f.work.claim("WinE", 16).unwrap().unwrap().attempt_id,
        attempt.attempt_id
    );
    f.report(
        &attempt,
        "stopped",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
    );
    assert_ne!(
        f.work.claim("WinE", 21).unwrap().unwrap().attempt_id,
        attempt.attempt_id
    );
}

#[test]
fn runner_claim_filters_both_new_and_previously_issued_assignments() {
    let mut f = Fixture::new();
    f.work
        .register_environment(
            &f.admin,
            &Environment {
                id: "second".into(),
                label: "別領域".into(),
                resource_id: "second-slot".into(),
                runner_id: "WinE".into(),
                capacity: 1,
                project_ids: vec!["alpha".into()],
            },
        )
        .unwrap();
    let first = f.submit("solver", "first", 0);
    let second = f.submit("second", "second", 0);
    let second_attempt = f
        .work
        .claim_for("WinE", Some(&["second".into()]), 12)
        .unwrap()
        .unwrap();
    assert_eq!(second_attempt.job.id, second.id);
    let first_attempt = f
        .work
        .claim_for("WinE", Some(&["solver".into()]), 13)
        .unwrap()
        .unwrap();
    assert_eq!(first_attempt.job.id, first.id);
    assert_eq!(
        f.work
            .claim_for("WinE", Some(&["second".into()]), 14)
            .unwrap()
            .unwrap()
            .attempt_id,
        second_attempt.attempt_id
    );
    assert_eq!(f.work.assignments("WinE").unwrap().len(), 2);
    assert!(f.work.claim_for("WinE", Some(&[]), 15).is_err());
    assert!(
        f.work
            .claim_for("WinE", Some(&["solver".into(), "solver".into()]), 15)
            .is_err()
    );
    assert!(
        first_attempt
            .allowed_child_environments
            .contains(&"second".into())
    );
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        16,
    )
    .unwrap();
    assert!(
        f.work
            .attempt("WinE", &first_attempt.attempt_id)
            .unwrap()
            .assignment
            .allowed_child_environments
            .is_empty()
    );
}

#[test]
fn version_two_migration_preserves_allocated_work_and_identity() {
    completion::released_migration(2);
}
fn child() -> ReportOutcome {
    ReportOutcome::YieldToChild {
        checkpoint: json!({"next":"consume-child","step":2}),
        child_environment_id: "solver".into(),
        child_title: "子の計算".into(),
        child_input: json!({"input_version":"v1"}),
        resources_released: true,
    }
}

fn request_approval(f: &mut Fixture, attempt: &Assignment, id: &str) {
    f.report(
        attempt,
        &format!("request-{id}"),
        ReportOutcome::ApprovalRequested {
            approval_id: id.into(),
            request: json!({"tool":"shell","arguments":{"command":"write one controlled file"}}),
            expires_at_ms: 1000,
        },
    );
}

#[test]
fn shared_approval_is_exact_authorized_durable_and_delivered_without_replacing_the_choice() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "needs-approval", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "approval-one");
    assert!(
        f.work
            .approval(&f.alice, &job.id, 21)
            .unwrap()
            .unwrap()
            .can_decide
    );
    assert!(f.work.approval(&f.admin, &job.id, 21).is_err());
    assert!(
        f.work
            .decide_approval(
                &f.bob,
                &job.id,
                "approval-one",
                ApprovalDecision::Approve,
                22
            )
            .is_err()
    );
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-one",
                &attempt.attempt_id,
                attempt.generation,
                23
            )
            .unwrap()
            .is_none()
    );
    f.work
        .decide_approval(
            &f.alice,
            &job.id,
            "approval-one",
            ApprovalDecision::Approve,
            24,
        )
        .unwrap();
    f.reopen();
    assert!(
        f.work
            .consume_approval(
                "WinC",
                "approval-one",
                &attempt.attempt_id,
                attempt.generation,
                25
            )
            .is_err()
    );
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-one",
                &attempt.attempt_id,
                attempt.generation + 1,
                25
            )
            .is_err()
    );
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-one",
                "different-attempt",
                attempt.generation,
                25
            )
            .is_err()
    );
    for now in [26, 27] {
        let answer = f
            .work
            .consume_approval(
                "WinE",
                "approval-one",
                &attempt.attempt_id,
                attempt.generation,
                now,
            )
            .unwrap()
            .unwrap();
        let ApprovalConsumeResult::Answer(answer) = answer else {
            panic!("A live approval must deliver the recorded decision");
        };
        assert_eq!(answer.approval_id, "approval-one");
        assert_eq!(answer.decision, ApprovalDecision::Approve);
    }
    assert_eq!(
        f.work
            .approval(&f.alice, &job.id, 28)
            .unwrap()
            .unwrap()
            .status,
        "consumed"
    );
    assert!(
        f.work
            .decide_approval(
                &f.alice,
                &job.id,
                "approval-one",
                ApprovalDecision::Deny,
                29
            )
            .is_err()
    );
    f.work.cancel(&f.alice, &job.id, 30).unwrap();
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-one",
                &attempt.attempt_id,
                attempt.generation,
                31
            )
            .is_err()
    );
}

#[test]
fn approval_expiry_and_revoked_project_role_never_deliver_an_old_approval() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "needs-approval", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "approval-two");
    f.work
        .decide_approval(
            &f.alice,
            &job.id,
            "approval-two",
            ApprovalDecision::Approve,
            21,
        )
        .unwrap();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        22,
    )
    .unwrap();
    assert!(
        !f.work
            .approval(&f.alice, &job.id, 23)
            .unwrap()
            .unwrap()
            .can_decide
    );
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-two",
                &attempt.attempt_id,
                attempt.generation,
                23
            )
            .is_err()
    );
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Contributor),
        24,
    )
    .unwrap();
    assert!(
        f.work
            .consume_approval(
                "WinE",
                "approval-two",
                &attempt.attempt_id,
                attempt.generation,
                1000
            )
            .is_err()
    );
    assert_eq!(
        f.work
            .approval(&f.alice, &job.id, 1000)
            .unwrap()
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        f.work.attempt("WinE", &attempt.attempt_id).unwrap().state,
        "running"
    );
    assert!(
        f.work
            .report(
                "WinE",
                &Report {
                    event_id: "reuse".into(),
                    attempt_id: attempt.attempt_id.clone(),
                    generation: attempt.generation,
                    outcome: ReportOutcome::ApprovalRequested {
                        approval_id: "approval-two".into(),
                        request: json!({}),
                        expires_at_ms: 2000
                    }
                },
                1001
            )
            .is_err()
    );
}

#[test]
fn acceptance_replay_is_durable_and_changed_content_conflicts() {
    let mut f = Fixture::new();
    let first = f.submit("solver", "request1", 2);
    f.reopen();
    assert_eq!(f.submit("solver", "request1", 2).id, first.id);
    let bad = SubmitJob {
        request_id: "request1".into(),
        project_id: "alpha".into(),
        environment_id: "solver".into(),
        title: "changed".into(),
        input: json!({}),
        descendant_budget: 2,
        start_before_ms: None,
    };
    assert!(matches!(
        f.work.submit(&f.alice, &bad, 11),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn project_privacy_is_not_replaced_by_administrator_or_device_authority() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "private", 0);
    assert!(f.work.job(&f.bob, &job.id).is_err());
    assert!(f.work.job(&f.admin, &job.id).is_err());
    assert!(f.work.list(&f.bob, "alpha", None, 100).is_err());
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 30).unwrap();
    assert!(f.work.job(&f.alice, &job.id).is_err());
    assert!(f.work.claim("WinE", 31).unwrap().is_none());
}

#[test]
fn projects_share_one_capacity_and_restart_does_not_release_unknown_execution() {
    let mut f = Fixture::new();
    let a = f.submit("solver", "a", 0);
    f.work
        .submit(
            &f.bob,
            &SubmitJob {
                request_id: "b".into(),
                project_id: "beta".into(),
                environment_id: "solver".into(),
                title: "b".into(),
                input: json!({}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            11,
        )
        .unwrap();
    let assigned = f.work.claim("WinE", 12).unwrap().unwrap();
    assert_eq!(assigned.job.id, a.id);
    assert_eq!(
        f.work.claim("WinE", 13).unwrap().unwrap().attempt_id,
        assigned.attempt_id
    );
    f.report(&assigned, "start", ReportOutcome::Started);
    f.reopen();
    assert!(f.work.claim("WinE", 10_000_000).unwrap().is_none());
    f.report(&assigned, "done", finished());
    assert_eq!(
        f.work
            .claim("WinE", 10_000_001)
            .unwrap()
            .unwrap()
            .job
            .project_id,
        "beta"
    );
}

#[test]
fn checkpoint_child_and_parent_wake_survive_reopen_and_duplicate_reports() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "root", 2);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    let waiting = f.report(&parent, "yield", child());
    assert_eq!(waiting.state, JobState::WaitingChild);
    assert_eq!(
        f.report(&parent, "yield", child()).awaiting_child_id,
        waiting.awaiting_child_id
    );
    f.reopen();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().checkpoint.unwrap()["step"],
        2
    );
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&child, "start", ReportOutcome::Started);
    f.report(&child, "done", finished());
    f.reopen();
    let continuation = f.work.claim("WinC", 23).unwrap().unwrap();
    assert_eq!(continuation.job.id, root.id);
    assert_eq!(continuation.generation, 2);
    assert_eq!(
        continuation
            .child_result
            .as_ref()
            .unwrap()
            .result
            .as_ref()
            .unwrap()["answer"],
        42
    );
    f.report(&child, "done", finished());
    assert_eq!(
        f.work.claim("WinC", 24).unwrap().unwrap().attempt_id,
        continuation.attempt_id
    );
    f.report(&continuation, "resume-start", ReportOutcome::Started);
    assert!(f.work.claim("WinC", 25).unwrap().is_none());
    assert_eq!(
        f.report(&continuation, "done", finished()).state,
        JobState::Succeeded
    );
}

#[test]
fn cancellation_during_child_wait_does_not_wake_parent_or_free_unconfirmed_slot() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "cancel", 1);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    assert_eq!(
        f.work.cancel(&f.alice, &root.id, 22).unwrap().state,
        JobState::Cancelling
    );
    f.reopen();
    assert!(f.work.claim("WinE", 23).unwrap().unwrap().stop_requested);
    assert!(
        f.work
            .report(
                "WinE",
                &Report {
                    event_id: "late-start".into(),
                    attempt_id: child.attempt_id.clone(),
                    generation: child.generation,
                    outcome: ReportOutcome::Started
                },
                24
            )
            .is_err()
    );
    assert!(f.work.claim("WinC", 25).unwrap().is_none());
    f.report(&child, "stopped", finished());
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
    assert!(f.work.claim("WinC", 26).unwrap().is_none());
}

#[test]
fn non_quiescent_checkpoint_and_result_keep_resource_occupied() {
    let mut f = Fixture::new();
    f.submit("parent", "busy", 1);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    for outcome in [
        ReportOutcome::Finished {
            success: true,
            result: json!({}),
            resources_released: false,
        },
        ReportOutcome::YieldToChild {
            checkpoint: json!({}),
            child_environment_id: "solver".into(),
            child_title: "child".into(),
            child_input: json!({}),
            resources_released: false,
        },
    ] {
        assert!(matches!(
            f.work.report(
                "WinC",
                &Report {
                    event_id: "unconfirmed".into(),
                    attempt_id: parent.attempt_id.clone(),
                    generation: parent.generation,
                    outcome
                },
                22
            ),
            Err(WorkError::Conflict)
        ));
    }
    assert_eq!(
        f.work.job(&f.alice, &parent.job.id).unwrap().state,
        JobState::Running
    );
    assert_eq!(f.work.list(&f.alice, "alpha", None, 100).unwrap().len(), 1);
}

#[test]
fn descendant_budget_is_cumulative_and_exhaustion_is_an_explicit_error() {
    let mut f = Fixture::new();
    f.submit("parent", "budget", 1);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let c = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&c, "start", ReportOutcome::Started);
    f.report(&c, "done", finished());
    let resumed = f.work.claim("WinC", 22).unwrap().unwrap();
    f.report(&resumed, "start", ReportOutcome::Started);
    assert!(matches!(
        f.work.report(
            "WinC",
            &Report {
                event_id: "another-child".into(),
                attempt_id: resumed.attempt_id,
                generation: resumed.generation,
                outcome: child()
            },
            23
        ),
        Err(WorkError::Capacity)
    ));
}

#[test]
fn wrong_worker_generation_and_event_payload_cannot_change_an_attempt() {
    let mut f = Fixture::new();
    f.submit("solver", "fenced", 0);
    let a = f.work.claim("WinE", 12).unwrap().unwrap();
    let mut report = Report {
        event_id: "start".into(),
        attempt_id: a.attempt_id,
        generation: a.generation,
        outcome: ReportOutcome::Started,
    };
    assert!(matches!(
        f.work.report("WinD", &report, 13),
        Err(WorkError::Forbidden)
    ));
    report.generation += 1;
    assert!(matches!(
        f.work.report("WinE", &report, 13),
        Err(WorkError::Conflict)
    ));
    report.generation -= 1;
    f.work.report("WinE", &report, 13).unwrap();
    report.outcome = finished();
    assert!(matches!(
        f.work.report("WinE", &report, 14),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn future_schema_is_rejected_without_rewriting_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite3");
    let db = Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 42).unwrap();
    assert!(matches!(SharedWork::open(&path), Err(WorkError::Conflict)));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        42
    );
}

#[test]
fn assigned_attempt_cannot_report_success_without_start_authorization() {
    let mut f = Fixture::new();
    f.submit("solver", "not-started", 0);
    let a = f.work.claim("WinE", 12).unwrap().unwrap();
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 13).unwrap();
    let report = Report {
        event_id: "unauthorized-success".into(),
        attempt_id: a.attempt_id.clone(),
        generation: a.generation,
        outcome: finished(),
    };
    assert!(matches!(
        f.work.report("WinE", &report, 14),
        Err(WorkError::Conflict)
    ));
    let failed = Report {
        event_id: "not-started-confirmed".into(),
        outcome: ReportOutcome::Finished {
            success: false,
            result: json!({"reason":"not_started"}),
            resources_released: true,
        },
        ..report
    };
    assert_eq!(
        f.work.report("WinE", &failed, 15).unwrap().state,
        JobState::Failed
    );
}

#[test]
fn admission_reserves_the_new_roots_uncreated_descendants_at_storage_limit() {
    let mut f = Fixture::new();
    let first = f.submit("solver", "retained", 0);
    let a = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&a, "start", ReportOutcome::Started);
    f.report(&a, "done", finished());
    f.work.db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<99998) INSERT INTO shared_jobs(id,root_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms) SELECT 'retained-'||x,'retained-'||x,project_id,environment_id,requestor_id,assignee_id,'retained-request-'||x,'{}','retained','{}','succeeded',0,1,1,1 FROM n,shared_jobs WHERE id=?1",[first.id]).unwrap();
    let request = SubmitJob {
        request_id: "last-slot".into(),
        project_id: "alpha".into(),
        environment_id: "solver".into(),
        title: "budget".into(),
        input: json!({}),
        descendant_budget: 1,
        start_before_ms: None,
    };
    assert!(matches!(
        f.work.submit(&f.alice, &request, 30),
        Err(WorkError::Capacity)
    ));
    assert!(
        f.work
            .submit(
                &f.alice,
                &SubmitJob {
                    descendant_budget: 0,
                    ..request
                },
                31
            )
            .is_ok()
    );
}

#[test]
fn runner_admission_keeps_every_active_attempt_reachable_for_stop() {
    let mut f = Fixture::new();
    f.work
        .register_environment(
            &f.admin,
            &Environment {
                id: "wide".into(),
                label: "capacity".into(),
                resource_id: "wide-resource".into(),
                runner_id: "WinE".into(),
                capacity: 128,
                project_ids: vec!["alpha".into()],
            },
        )
        .unwrap();
    let mut active = Vec::new();
    for n in 0..128 {
        f.submit("wide", &format!("wide-{n}"), 0);
        let assignment = f.work.claim("WinE", 30 + n).unwrap().unwrap();
        f.report(&assignment, "start", ReportOutcome::Started);
        active.push(assignment);
    }
    let waiting = f.submit("solver", "one-more-resource", 0);
    assert!(f.work.claim("WinE", 200).unwrap().is_none());
    assert_eq!(f.work.assignments("WinE").unwrap().len(), 128);
    let last = &active[127];
    f.work.cancel(&f.alice, &last.job.id, 201).unwrap();
    assert!(
        f.work
            .assignments("WinE")
            .unwrap()
            .iter()
            .any(|a| a.attempt_id == last.attempt_id && a.stop_requested)
    );
    f.report(last, "stopped", finished());
    assert_eq!(
        f.work.claim("WinE", 202).unwrap().unwrap().job.id,
        waiting.id
    );
}

mod approval_reconfirmation;
mod completion;
mod controller_projection;
mod data_tests;
mod deadlines;
mod participation;
mod project_setup;
mod runner_contact;
