use super::*;
use rusqlite::OptionalExtension;
fn command(f: &Fixture, request: ManagementRequest) -> ManagementCommand {
    ManagementCommand {
        request_id: ulid::Ulid::new().to_string(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request,
    }
}
fn manage(f: &mut Fixture, request: ManagementRequest) -> Value {
    let command = command(f, request);
    f.work.manage(&f.admin, &command, 40).unwrap()
}
fn add_bob(f: &mut Fixture) {
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        30,
    )
    .unwrap();
}
#[test]
fn password_receipt_does_not_add_a_fast_offline_password_verifier() {
    use sha2::{Digest, Sha256};
    let mut f = Fixture::new();
    let request = command(
        &f,
        ManagementRequest::SetPassword {
            user_id: f.alice.user_id.clone(),
            password: "new password secret long enough".into(),
        },
    );
    f.work.manage(&f.admin, &request, 40).unwrap();
    let stored:String=f.work.db.query_row("SELECT payload_hash FROM shared_management_receipts WHERE actor_id=?1 AND request_id=?2",rusqlite::params![f.admin.user_id,request.request_id],|r|r.get(0)).unwrap();
    assert_ne!(
        stored,
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&request).unwrap())
        )
    );
    assert!(stored.starts_with("pbkdf2_sha256:"));
    f.reopen();
    assert!(f.work.manage(&f.admin, &request, 41).is_ok());
    let mut changed = request;
    changed.request = ManagementRequest::SetPassword {
        user_id: f.alice.user_id.clone(),
        password: "another different password".into(),
    };
    assert!(matches!(
        f.work.manage(&f.admin, &changed, 42),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn completion_scope_regression_environment_binding_cannot_widen_an_existing_roots_unused_ceiling() {
    let mut f = Fixture::new();
    f.submit("parent", "future-child-ceiling", 1);
    // Solver is authorized for a future child but has no job/attempt yet.
    let request = command(
        &f,
        ManagementRequest::UpdateEnvironment {
            id: "solver".into(),
            label: "Repurposed".into(),
            resource_id: "parent-slot".into(),
            runner_id: "WinF".into(),
            enabled: true,
            project_ids: vec!["alpha".into()],
        },
    );
    assert!(matches!(
        f.work.manage(&f.admin, &request, 30),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn completion_scope_regression_reader_requestor_notification_is_not_actionable() {
    let mut f = Fixture::new();
    add_bob(&mut f);
    f.submit("solver", "reader-notification", 0);
    let attempt = f.work.claim("WinE", 32).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "new-assignee-approval");
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        31,
    )
    .unwrap();
    let inbox = f.work.inbox(&f.alice, None, 20, 33).unwrap();
    let approval = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "approval")
        .unwrap();
    assert_eq!(approval["can_act"], false);
    assert_eq!(approval["approval_status"], "pending");
    assert_eq!(approval["approval_decision"], Value::Null);
}

#[test]
fn inbox_approval_status_tracks_each_exact_request_without_marking_it_read() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "notification-status", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "approval-first");
    let notification = |f: &Fixture, id: &str, now| {
        f.work.inbox(&f.alice, None, 20, now).unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["approval_id"] == id)
            .unwrap()
            .clone()
    };
    let pending = notification(&f, "approval-first", 21);
    assert_eq!(pending["approval_status"], "pending");
    assert_eq!(pending["can_act"], true);
    f.work
        .decide_approval(
            &f.alice,
            &job.id,
            "approval-first",
            ApprovalDecision::Approve,
            22,
        )
        .unwrap();
    let decided = notification(&f, "approval-first", 23);
    assert_eq!(decided["approval_status"], "decided");
    assert_eq!(decided["approval_decision"], "approve");
    assert_eq!(decided["can_act"], false);
    assert_eq!(decided["read_at_ms"], Value::Null);
    f.work
        .consume_approval(
            "WinE",
            "approval-first",
            &attempt.attempt_id,
            attempt.generation,
            24,
        )
        .unwrap();
    request_approval(&mut f, &attempt, "approval-second");
    assert_eq!(
        notification(&f, "approval-first", 25)["approval_status"],
        "consumed"
    );
    assert_eq!(
        notification(&f, "approval-second", 25)["approval_status"],
        "pending"
    );
    assert_eq!(
        f.work.inbox(&f.alice, None, 20, 25).unwrap()["unread_count"],
        2
    );
    let expired = notification(&f, "approval-second", 1000);
    assert_eq!(expired["approval_status"], "expired");
    assert_eq!(expired["can_act"], false);
    assert_eq!(expired["approval_decision"], Value::Null);
}

#[test]
fn cancelled_job_removes_notification_action_even_while_attempt_is_running() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "notification-cancel", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "cancelled-approval");
    f.work.cancel(&f.alice, &job.id, 21).unwrap();
    let inbox = f.work.inbox(&f.alice, None, 20, 22).unwrap();
    let approval = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["approval_id"] == "cancelled-approval")
        .unwrap();
    assert_eq!(approval["approval_status"], "cancelled");
    assert_eq!(approval["can_act"], false);
    assert_eq!(approval["read_at_ms"], Value::Null);
}

#[test]
fn groups_apply_only_in_the_granted_project_and_admin_has_no_business_bypass() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "group-readable", 0);
    manage(
        &mut f,
        ManagementRequest::CreateGroup {
            id: "planning".into(),
            label: "Planning".into(),
        },
    );
    let bob = f.bob.user_id.clone();
    manage(
        &mut f,
        ManagementRequest::GroupMember {
            group_id: "planning".into(),
            user_id: bob.clone(),
            member: true,
        },
    );
    manage(
        &mut f,
        ManagementRequest::GroupProjectRole {
            project_id: "alpha".into(),
            group_id: "planning".into(),
            role: Some(ProjectRole::Reader),
        },
    );
    assert!(f.work.job(&f.bob, &job.id).is_ok());
    assert!(f.work.cancel(&f.bob, &job.id, 41).is_err());
    assert!(f.work.job(&f.admin, &job.id).is_err());
    let projection = f.work.status(&f.bob, "alpha", None, 50, None).unwrap();
    assert_eq!(projection.jobs.len(), 1);
    // Direct and group roles resolve to the strongest grant within this project.
    manage(
        &mut f,
        ManagementRequest::GroupProjectRole {
            project_id: "alpha".into(),
            group_id: "planning".into(),
            role: Some(ProjectRole::Manager),
        },
    );
    assert_eq!(
        identity::require_project_role(&f.work.db, &bob, "alpha", ProjectRole::Reader).unwrap(),
        ProjectRole::Manager
    );
    manage(
        &mut f,
        ManagementRequest::UpdateGroup {
            id: "planning".into(),
            label: "Planning".into(),
            disabled: true,
        },
    );
    assert!(f.work.job(&f.bob, &job.id).is_err());
    assert_eq!(
        identity::require_project_role(&f.work.db, &bob, "beta", ProjectRole::Reader).unwrap(),
        ProjectRole::Contributor
    );
    f.reopen();
    assert!(f.work.job(&f.bob, &job.id).is_err());
}

#[test]
fn management_cas_and_exact_replay_preserve_metadata_and_referenced_entities() {
    let mut f = Fixture::new();
    let request = command(
        &f,
        ManagementRequest::CreateGroup {
            id: "one".into(),
            label: "One".into(),
        },
    );
    let result = f.work.manage(&f.admin, &request, 40).unwrap();
    f.reopen();
    assert_eq!(f.work.manage(&f.admin, &request, 41).unwrap(), result);
    let mut changed = request.clone();
    changed.request = ManagementRequest::CreateGroup {
        id: "two".into(),
        label: "Two".into(),
    };
    assert!(matches!(
        f.work.manage(&f.admin, &changed, 42),
        Err(WorkError::Conflict)
    ));
    changed.request_id = ulid::Ulid::new().to_string();
    assert!(matches!(
        f.work.manage(&f.admin, &changed, 43),
        Err(WorkError::Conflict)
    ));
    let job = f.submit("solver", "audit-kept", 0);
    for mutation in [
        ManagementRequest::DeleteUser {
            user_id: f.alice.user_id.clone(),
        },
        ManagementRequest::DeleteEnvironment {
            id: "solver".into(),
        },
        ManagementRequest::DeleteResource {
            id: "wine-slot".into(),
        },
    ] {
        let request = command(&f, mutation);
        assert!(f.work.manage(&f.admin, &request, 44).is_err());
    }
    assert_eq!(f.work.job(&f.alice, &job.id).unwrap().title, "audit-kept");
    let snapshot = f.work.management_snapshot(&f.admin).unwrap().to_string();
    assert!(!snapshot.contains("audit-kept"));
    assert!(!snapshot.contains("password"));
}

#[test]
fn retired_handover_has_no_effect_on_assignment_or_running_approval() {
    for running in [false, true] {
        let mut f = Fixture::new();
        add_bob(&mut f);
        let root = f.submit("solver", "retired-handover", 0);
        let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
        if running {
            f.report(&attempt, "start", ReportOutcome::Started);
            request_approval(&mut f, &attempt, "pending-approval");
        }
        let before = f.work.job(&f.alice, &root.id).unwrap();
        assert!(matches!(
            f.work.handover(
                &f.alice,
                &root.id,
                &HandoverRequest {
                    expected_revision: before.revision,
                    new_assignee_id: f.bob.user_id.clone()
                },
                21
            ),
            Err(WorkError::Retired)
        ));
        let after = f.work.job(&f.alice, &root.id).unwrap();
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap()
        );
        assert_eq!(
            f.work.attempt("WinE", &attempt.attempt_id).unwrap().state,
            if running { "running" } else { "assigned" }
        );
        if running {
            assert!(
                f.work
                    .approval(&f.alice, &root.id, 22)
                    .unwrap()
                    .unwrap()
                    .can_decide
            );
        }
    }
}

#[test]
fn retired_handover_does_not_block_child_delegation_or_parent_reentry() {
    let mut f = Fixture::new();
    add_bob(&mut f);
    let root = f.submit("parent", "retired-delegation", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
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
    f.report(&parent, "yield", child());
    let child = f.work.claim("WinE", 22).unwrap().unwrap();
    assert_eq!(child.job.requestor_id, f.alice.user_id);
    assert_eq!(child.job.assignee_id, f.alice.user_id);
    f.report(&child, "child-start", ReportOutcome::Started);
    f.report(&child, "child-done", finished());
    assert_eq!(f.work.claim("WinC", 23).unwrap().unwrap().job.id, root.id);
}

#[test]
fn delivered_approval_is_revalidated_at_effect_after_invalidation_or_credential_change() {
    for change_credentials in [false, true] {
        let mut f = Fixture::new();
        add_bob(&mut f);
        let root = f.submit("solver", "approve-effect", 0);
        let attempt = f.work.claim("WinE", 40).unwrap().unwrap();
        f.report(&attempt, "start", ReportOutcome::Started);
        request_approval(&mut f, &attempt, "exact-approval");
        f.work
            .decide_approval(
                &f.alice,
                &root.id,
                "exact-approval",
                ApprovalDecision::Approve,
                41,
            )
            .unwrap();
        f.work
            .consume_approval(
                "WinE",
                "exact-approval",
                &attempt.attempt_id,
                attempt.generation,
                42,
            )
            .unwrap()
            .unwrap();
        assert!(
            f.work
                .authorize_effect(
                    "WinE",
                    &attempt.attempt_id,
                    attempt.generation,
                    1,
                    Some("exact-approval"),
                    43
                )
                .is_ok()
        );
        if change_credentials {
            identity::set_password(
                &f.work.db,
                &f.admin,
                &f.alice.user_id,
                "a different password long enough",
                44,
            )
            .unwrap();
        } else {
            f.work
                .db
                .execute(
                    "UPDATE shared_approvals SET invalidated_at_ms=44 WHERE id='exact-approval'",
                    [],
                )
                .unwrap();
        }
        assert!(
            f.work
                .authorize_effect(
                    "WinE",
                    &attempt.attempt_id,
                    attempt.generation,
                    1,
                    Some("exact-approval"),
                    45
                )
                .is_err()
        );
        let consumed = f.work.consume_approval(
            "WinE",
            "exact-approval",
            &attempt.attempt_id,
            attempt.generation,
            45,
        );
        if change_credentials {
            assert!(consumed.is_err());
        } else {
            assert!(matches!(
                consumed.unwrap(),
                Some(ApprovalConsumeResult::ReconfirmationRequired {
                    reconfirmation_required: true,
                    ..
                })
            ));
        }
    }
}

#[test]
fn inbox_filters_counts_and_read_receipts_by_current_human_project_grant() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "notice", 0);
    let attempt = f.work.claim("WinE", 30).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(&attempt, "done", finished());
    let inbox = f.work.inbox(&f.alice, None, 10, 50).unwrap();
    assert_eq!(inbox["unread_count"], 1);
    assert_eq!(
        f.work.inbox(&f.admin, None, 10, 50).unwrap()["unread_count"],
        0
    );
    assert_eq!(
        f.work.inbox(&f.bob, None, 10, 50).unwrap()["unread_count"],
        0
    );
    let id = inbox["items"][0]["id"].as_str().unwrap();
    assert!(f.work.read_inbox(&f.bob, id, 51).is_err());
    f.work.read_inbox(&f.alice, id, 51).unwrap();
    f.reopen();
    assert_eq!(
        f.work.inbox(&f.alice, None, 10, 52).unwrap()["unread_count"],
        0
    );
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 53).unwrap();
    assert!(
        f.work.inbox(&f.alice, None, 10, 54).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(f.work.job(&f.alice, &job.id).is_err());
}

#[test]
fn recovery_requires_exact_attempt_generation_and_explicit_stop_attestation() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "unknown", 0);
    let attempt = f.work.claim("WinE", 30).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(
        &attempt,
        "unknown",
        ReportOutcome::Uncertain {
            reason: "process proof unavailable".into(),
        },
    );
    for (generation, ack) in [(attempt.generation, false), (attempt.generation + 1, true)] {
        let request = command(
            &f,
            ManagementRequest::SettleAttempt {
                attempt_id: attempt.attempt_id.clone(),
                generation,
                acknowledge_stopped: ack,
                reason: "operator inspected the stopped host".into(),
            },
        );
        assert!(f.work.manage(&f.admin, &request, 40).is_err());
        assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
    }
    manage(
        &mut f,
        ManagementRequest::SettleAttempt {
            attempt_id: attempt.attempt_id.clone(),
            generation: attempt.generation,
            acknowledge_stopped: true,
            reason: "operator inspected the stopped host".into(),
        },
    );
    assert_eq!(
        f.work.job(&f.alice, &job.id).unwrap().state,
        JobState::Failed
    );
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    assert!(
        f.work
            .report(
                "WinE",
                &Report {
                    event_id: "late-success".into(),
                    attempt_id: attempt.attempt_id,
                    generation: attempt.generation,
                    outcome: finished()
                },
                41
            )
            .is_err()
    );
}

pub(super) fn released_migration(version: u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("released.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../../../migrations/V1__shared_work.sql"))
        .unwrap();
    identity::migrate(&db).unwrap();
    let admin = identity::bootstrap_admin(&db, "admin", "Admin", "test-password-12345", 1).unwrap();
    db.execute_batch("INSERT INTO shared_projects VALUES('old','Old'); INSERT INTO shared_resources VALUES('slot',1); INSERT INTO shared_environments VALUES('env','Env','slot','WinE',1); INSERT INTO shared_environment_projects VALUES('env','old');").unwrap();
    identity::set_project_role(
        &db,
        &admin,
        "old",
        &admin.user_id,
        Some(ProjectRole::Contributor),
        1,
    )
    .unwrap();
    db.execute("INSERT INTO shared_jobs(id,root_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms) VALUES('job','job','old','env',?1,?1,'request','{}','Old job','{}','assigned',0,1,1,1)",[&admin.user_id]).unwrap();
    db.execute(
        "INSERT INTO shared_attempts VALUES('attempt','job','WinE','slot',1,'assigned',1,1)",
        [],
    )
    .unwrap();
    if version >= 3 {
        db.execute_batch(include_str!(
            "../../../migrations/V3__shared_attempt_observation.sql"
        ))
        .unwrap();
    }
    if version >= 4 {
        db.execute_batch(include_str!("../../../migrations/V4__shared_approvals.sql"))
            .unwrap();
        db.execute("INSERT INTO shared_approvals(id,attempt_id,request_json,expires_at_ms,created_at_ms) VALUES('old-approval','attempt','{}',1000,1)",[]).unwrap();
    }
    for (target, sql) in [
        (
            5,
            include_str!("../../../migrations/V5__shared_management.sql"),
        ),
        (6, include_str!("../../../migrations/V6__shared_data.sql")),
        (
            7,
            include_str!("../../../migrations/V7__shared_runner_participation.sql"),
        ),
        (
            8,
            include_str!("../../../migrations/V8__shared_data_retention.sql"),
        ),
        (
            9,
            include_str!("../../../migrations/V9__shared_managed_files.sql"),
        ),
        (
            10,
            include_str!("../../../migrations/V10__shared_start_deadlines.sql"),
        ),
        (
            11,
            include_str!("../../../migrations/V11__remembered_human_sessions.sql"),
        ),
    ] {
        if version >= target {
            db.execute_batch(sql).unwrap();
        }
    }
    db.pragma_update(None, "user_version", version).unwrap();
    drop(db);
    let mut work = SharedWork::open(&path).unwrap();
    assert!(
        work.require_project_controller("old", "new-registered-PC")
            .is_ok()
    );
    let device_projects = work.device_projects("WinE").unwrap();
    assert_eq!(device_projects["projects"][0]["environment_id"], "env");
    assert_eq!(device_projects["projects"][0]["can_control"], true);
    let attempt = work.attempt("WinE", "attempt").unwrap();
    assert_eq!(attempt.assignment.job.title, "Old job");
    assert_eq!(attempt.assignment.job.quota_owner_id, admin.user_id);
    assert_eq!(attempt.assignment.authority_generation, 1);
    work.report(
        "WinE",
        &Report {
            event_id: "start".into(),
            attempt_id: "attempt".into(),
            generation: 1,
            outcome: ReportOutcome::Started,
        },
        10,
    )
    .unwrap();
    assert_eq!(
        identity::active_principal(&work.db, &admin.user_id).unwrap(),
        admin
    );
    if version >= 4 {
        assert_eq!(
            work.approval(&admin, "job", 11).unwrap().unwrap().status,
            "cancelled"
        );
    }
    assert_eq!(
        work.db
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .unwrap(),
        None
    );
}
#[test]
fn version_four_forward_migration_preserves_work_and_invalidates_unproven_old_approval() {
    released_migration(4)
}
