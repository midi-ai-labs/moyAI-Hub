use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

fn local_turn(request: &str, origin: &str, turn: &str, epoch: u64) -> SubmitJob {
    SubmitJob {
        request_id: request.into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: "solver".into(),
        origin_session_ref: Some(origin.into()),
        origin_turn_ref: Some(turn.into()),
        origin_turn_epoch: Some(epoch),
        title: request.into(),
        input: json!({"prompt":request}),
        descendant_budget: 1,
        start_before_ms: None,
    }
}

fn epoch(value: &serde_json::Value) -> u64 {
    value["epoch"].as_u64().unwrap()
}

#[test]
fn scoped_stop_fences_late_admission_but_a_new_local_turn_can_use_the_same_session() {
    let mut f = Fixture::new();
    assert!(matches!(
        f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-1", "admit-1", false, 1, 10),
        Err(WorkError::Conflict)
    ));
    let first = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-1", "admit-1", true, 1, 10)
            .unwrap(),
    );
    assert_eq!(first, 1);
    assert_eq!(
        epoch(
            &f.work
                .admit_origin_turn(&f.alice, "WinA", "chat", "turn-1", "retry", false, 1, 11)
                .unwrap()
        ),
        first
    );
    assert!(matches!(
        f.work.admit_origin_turn(
            &f.alice,
            "WinA",
            "chat",
            "turn-1",
            "wrong-revision",
            true,
            2,
            11
        ),
        Err(WorkError::Conflict)
    ));
    let preview = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("preview", "chat", "turn-1", first),
            12,
            Some("WinA"),
        )
        .unwrap();
    let assignment = f.work.claim("WinE", 13).unwrap().unwrap();
    f.report(&assignment, "start", ReportOutcome::Started);
    f.report(
        &assignment,
        "preview-retain",
        ReportOutcome::RetainService {
            service_id: "preview-service".into(),
            expires_at_ms: 1000,
        },
    );
    f.report(&assignment, "finish", finished());

    let second = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-2", "admit-2", true, 2, 21)
            .unwrap(),
    );
    let queued = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("queued", "chat", "turn-2", second),
            21,
            Some("WinA"),
        )
        .unwrap();
    f.work
        .stop_origin_turn(&f.alice, "WinA", "chat", "turn-2", 2, "stop-turn-2", 22)
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Cancelled
    );
    assert!(!f.work.runner_services("WinE", 22).unwrap()[0].stop_requested);
    assert!(matches!(
        f.work.submit_for_device(
            &f.alice,
            &local_turn("late", "chat", "turn-2", second),
            23,
            Some("WinA")
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        f.work.admit_origin_turn(
            &f.alice,
            "WinA",
            "chat",
            "turn-2",
            "late-admit",
            true,
            2,
            23
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        f.work.submit_for_device(
            &f.alice,
            &local_turn("wrong-device", "chat", "turn-1", first),
            23,
            Some("WinB")
        ),
        Err(WorkError::Conflict)
    ));
    let mut unbound = local_turn("unbound", "chat", "turn-1", first);
    unbound.origin_turn_ref = None;
    unbound.origin_turn_epoch = None;
    assert!(matches!(
        f.work
            .submit_for_device(&f.alice, &unbound, 23, Some("WinA")),
        Err(WorkError::Conflict)
    ));

    f.work
        .stop_origin(&f.alice, "WinA", "chat", 2, "all-stop", 24)
        .unwrap();
    assert!(matches!(
        f.work.admit_origin_turn(
            &f.alice,
            "WinA",
            "chat",
            "delayed-old-turn",
            "delayed-admit",
            true,
            2,
            24
        ),
        Err(WorkError::Conflict)
    ));
    assert!(f.work.runner_services("WinE", 24).unwrap()[0].stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    let third = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-3", "admit-3", true, 3, 25)
            .unwrap(),
    );
    assert!(third > second);
    let resumed = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("resumed", "chat", "turn-3", third),
            25,
            Some("WinA"),
        )
        .unwrap();
    assert!(f.work.claim("WinE", 25).unwrap().is_none());
    // A retry of the old receipt cannot sweep the new turn.
    f.work
        .stop_origin(&f.alice, "WinA", "chat", 2, "all-stop", 26)
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &resumed.id).unwrap().state,
        JobState::Queued
    );
    assert!(matches!(
        f.work
            .stop_origin(&f.alice, "WinA", "chat", 3, "all-stop", 26),
        Err(WorkError::Conflict)
    ));
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "preview-stopped".into(),
                attempt_id: assignment.attempt_id.clone(),
                generation: assignment.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "preview-service".into(),
                },
            },
            27,
        )
        .unwrap();
    assert!(f.work.runner_services("WinE", 27).unwrap().is_empty());
    assert_eq!(
        f.work.claim("WinE", 28).unwrap().unwrap().job.id,
        resumed.id
    );
    assert_eq!(preview.id, assignment.job.id);
}

#[test]
fn active_turn_stop_blocks_late_retention_and_child_creation_until_runner_cleanup() {
    let mut f = Fixture::new();
    let epoch = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-1", "admit", true, 1, 10)
            .unwrap(),
    );
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("active", "chat", "turn-1", epoch),
            10,
            Some("WinA"),
        )
        .unwrap();
    let assignment = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&assignment, "started", ReportOutcome::Started);
    f.work
        .stop_origin_turn(&f.alice, "WinA", "chat", "turn-1", 1, "stop", 21)
        .unwrap();
    assert!(f.work.claim("WinE", 22).unwrap().unwrap().stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    for (event, outcome) in [
        (
            "late-retain",
            ReportOutcome::RetainService {
                service_id: "late".into(),
                expires_at_ms: 1000,
            },
        ),
        (
            "late-child",
            ReportOutcome::YieldToChild {
                checkpoint: json!({"step":1}),
                child_environment_id: "solver".into(),
                child_title: "late child".into(),
                child_input: json!({"prompt":"late"}),
                resources_released: true,
            },
        ),
    ] {
        assert!(matches!(
            f.work.report(
                "WinE",
                &Report {
                    event_id: event.into(),
                    attempt_id: assignment.attempt_id.clone(),
                    generation: assignment.generation,
                    outcome,
                },
                22
            ),
            Err(WorkError::Conflict)
        ));
    }
    let ended = f
        .work
        .report(
            "WinE",
            &Report {
                event_id: "cleanup".into(),
                attempt_id: assignment.attempt_id.clone(),
                generation: assignment.generation,
                outcome: finished(),
            },
            23,
        )
        .unwrap();
    assert_eq!(ended.state, JobState::Cancelled);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        0
    );
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
}

#[test]
fn origin_stop_is_owned_by_the_original_person_and_exact_controller_device() {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        1,
    )
    .unwrap();
    let epoch = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "private", "turn-1", "admit", true, 1, 10)
            .unwrap(),
    );
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("alice-owned", "private", "turn-1", epoch),
            10,
            Some("WinA"),
        )
        .unwrap();
    f.work
        .handover(
            &f.alice,
            &root.id,
            &HandoverRequest {
                expected_revision: root.revision,
                new_assignee_id: f.bob.user_id.clone(),
            },
            11,
        )
        .unwrap();
    let assignment = f.work.claim("WinE", 12).unwrap().unwrap();
    assert_eq!(assignment.job.assignee_id, f.bob.user_id);
    f.report(&assignment, "start", ReportOutcome::Started);
    // The opaque session/turn strings alone are not authority.
    f.work
        .stop_origin_turn(&f.bob, "WinA", "private", "turn-1", 1, "bob-stop", 21)
        .unwrap();
    f.work
        .stop_origin_turn(
            &f.alice,
            "WinB",
            "private",
            "turn-1",
            1,
            "wrong-device-stop",
            21,
        )
        .unwrap();
    assert!(
        !f.work
            .attempt("WinE", &assignment.attempt_id)
            .unwrap()
            .assignment
            .stop_requested
    );
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 22).unwrap();
    assert!(matches!(
        f.work
            .stop_origin_turn(&f.alice, "WinA", "private", "turn-1", 1, "alice-stop", 23),
        Err(WorkError::Identity(_))
    ));
    assert!(
        !f.work
            .attempt("WinE", &assignment.attempt_id)
            .unwrap()
            .assignment
            .stop_requested
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
    f.work
        .stop_origin_turn(&f.alice, "WinA", "private", "turn-1", 1, "alice-stop", 25)
        .unwrap();
    assert!(
        f.work
            .attempt("WinE", &assignment.attempt_id)
            .unwrap()
            .assignment
            .stop_requested
    );
}

#[test]
fn project_conversation_stop_fences_continuation_and_history_is_stable_and_authorized() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "project-chat", 0);
    let first = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&first, "start", ReportOutcome::Started);
    let archive = json!({"version":1,"job_id":root.id,"project_id":"alpha","environment_id":"solver","transcript":[]});
    let bytes = serde_json::to_vec(&archive).unwrap();
    f.work
        .runner_upload_asset(
            "WinE",
            &first.attempt_id,
            &RunnerAssetUpload {
                generation: first.generation,
                kind: "archive".into(),
                base_sha256: None,
                upload: AssetUpload {
                    request_id: "archive".into(),
                    name: "history.json".into(),
                    content_base64: STANDARD.encode(&bytes),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                },
            },
            19,
        )
        .unwrap();
    f.report(
        &first,
        "retained",
        ReportOutcome::RetainService {
            service_id: "project-preview".into(),
            expires_at_ms: 1000,
        },
    );
    f.report(&first, "done", finished());
    let prior = f.work.job(&f.alice, &root.id).unwrap();
    let continuation = f
        .work
        .continue_job(
            &f.alice,
            &root.id,
            &ContinueJob {
                request_id: "continue".into(),
                expected_revision: prior.revision,
                project_participation: None,
                conversation_epoch: None,
                prompt: "next".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            21,
        )
        .unwrap();
    let page = f
        .work
        .conversation_history(&f.alice, "alpha", &root.id, None, None, 1, 22)
        .unwrap();
    assert_eq!(page.jobs[0].job.id, continuation.id);
    assert_eq!(page.conversation_epoch, 0);
    assert_eq!(page.jobs[0].input["prompt"], "next");
    assert!(page.next_before.is_some());
    // A root appended after the first page belongs to the next refresh, not
    // to the frozen older pages.
    f.work.db.execute("INSERT INTO shared_jobs(id,root_id,conversation_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms) SELECT 'later-root','later-root',?2,project_id,environment_id,requestor_id,assignee_id,'later-request','{}','later','{}','queued',0,1,23,23 FROM shared_jobs WHERE id=?1",rusqlite::params![continuation.id,root.id]).unwrap();
    let older = f
        .work
        .conversation_history(
            &f.alice,
            "alpha",
            &root.id,
            Some(page.snapshot),
            page.next_before,
            1,
            22,
        )
        .unwrap();
    assert_eq!(older.jobs[0].job.id, root.id);
    assert!(older.next_before.is_none());
    assert!(matches!(
        f.work
            .conversation_history(&f.bob, "alpha", &root.id, None, None, 1, 22),
        Err(WorkError::Identity(_))
    ));
    assert!(matches!(
        f.work
            .conversation_history(&f.alice, "beta", &root.id, None, None, 1, 22),
        Err(WorkError::Identity(_))
    ));
    assert!(matches!(
        f.work
            .conversation_history(&f.alice, "alpha", &root.id, None, Some(1), 1, 22),
        Err(WorkError::Invalid)
    ));
    assert!(matches!(
        f.work
            .conversation_history(&f.alice, "alpha", &root.id, None, None, 51, 22),
        Err(WorkError::Invalid)
    ));
    f.work
        .db
        .execute("DELETE FROM shared_jobs WHERE id='later-root'", [])
        .unwrap();

    assert!(matches!(
        f.work
            .stop_project_conversation(&f.bob, "alpha", &root.id, "bob-stop", 22),
        Err(WorkError::Identity(_))
    ));
    let first_stop = f
        .work
        .stop_project_conversation(&f.alice, "alpha", &root.id, "stop-project", 23)
        .unwrap();
    assert_eq!(first_stop["conversation_epoch"], 1);
    assert!(f.work.runner_services("WinE", 23).unwrap()[0].stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    assert_eq!(
        f.work.job(&f.alice, &continuation.id).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        f.work.job_detail_at(&f.alice, &root.id, 24).unwrap()["can_continue"],
        false
    );
    assert_eq!(
        f.work.job_detail_at(&f.alice, &root.id, 24).unwrap()["conversation_epoch"],
        1
    );
    assert!(matches!(
        f.work.continue_job(
            &f.alice,
            &root.id,
            &ContinueJob {
                request_id: "late-continue".into(),
                expected_revision: prior.revision,
                project_participation: None,
                conversation_epoch: None,
                prompt: "late".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            25
        ),
        Err(WorkError::Conflict)
    ));
    // A stopped newest root does not make an older archived root current again.
    assert!(matches!(
        f.work.continue_job(
            &f.alice,
            &root.id,
            &ContinueJob {
                request_id: "old-root-after-stop".into(),
                expected_revision: prior.revision,
                project_participation: None,
                conversation_epoch: Some(1),
                prompt: "old turn".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            26
        ),
        Err(WorkError::Conflict)
    ));
    let first_retry = f
        .work
        .stop_project_conversation(&f.alice, "alpha", &root.id, "stop-project", 26)
        .unwrap();
    assert_eq!(first_retry["conversation_epoch"], 1);
    let second_stop = f
        .work
        .stop_project_conversation(&f.alice, "alpha", &root.id, "stop-project-other", 27)
        .unwrap();
    assert_eq!(second_stop["conversation_epoch"], 2);
    assert!(matches!(
        f.work.continue_job(
            &f.alice,
            &root.id,
            &ContinueJob {
                request_id: "stale-first-epoch".into(),
                expected_revision: prior.revision,
                project_participation: None,
                conversation_epoch: Some(1),
                prompt: "late old turn".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            28
        ),
        Err(WorkError::Conflict)
    ));
    assert_eq!(
        f.work
            .conversation_history(&f.alice, "alpha", &root.id, None, None, 1, 30)
            .unwrap()
            .conversation_epoch,
        2
    );
}

#[test]
fn v20_forward_migration_preserves_existing_origin_work_without_inventing_a_turn() {
    let mut f = Fixture::new();
    let legacy = local_turn("legacy", "old-chat", "unused", 1);
    let mut legacy = legacy;
    legacy.origin_turn_ref = None;
    legacy.origin_turn_epoch = None;
    let root = f
        .work
        .submit_for_device(&f.alice, &legacy, 10, Some("WinA"))
        .unwrap();
    f.work
        .db
        .execute_batch(
            "DROP TABLE shared_conversation_details;
         DROP INDEX shared_jobs_controller_participation;
         ALTER TABLE shared_jobs DROP COLUMN controller_device_id;
         ALTER TABLE shared_jobs DROP COLUMN controller_participation_generation; ALTER TABLE shared_jobs DROP COLUMN revises_job_id;
         ALTER TABLE shared_projects DROP COLUMN deleted_at_ms;
         ALTER TABLE shared_project_devices DROP COLUMN participation_generation;
         DROP INDEX shared_jobs_origin_turn;
         DROP INDEX shared_jobs_conversation_history;
         DROP TABLE shared_project_stop_receipts;
         DROP TABLE shared_conversation_stop_fences;
         DROP TABLE shared_origin_turns;
         DROP TABLE shared_origin_scopes;
         ALTER TABLE shared_jobs DROP COLUMN origin_turn_revision;
         ALTER TABLE shared_jobs DROP COLUMN origin_turn_epoch;
         ALTER TABLE shared_jobs DROP COLUMN origin_turn_ref;
         PRAGMA user_version=20;",
        )
        .unwrap();
    f.reopen();
    let version: u32 = f
        .work
        .db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 22);
    assert_eq!(
        f.work
            .job(&f.alice, &root.id)
            .unwrap()
            .origin_session_ref
            .as_deref(),
        Some("old-chat")
    );
    let revision: Option<u64> = f
        .work
        .db
        .query_row(
            "SELECT origin_turn_revision FROM shared_jobs WHERE id=?1",
            [&root.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(revision, None);
    f.work
        .stop_origin(&f.alice, "WinA", "old-chat", 1, "stop-migrated", 21)
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
    assert!(matches!(
        f.work.submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "legacy-late".into(),
                ..legacy
            },
            22,
            Some("WinA")
        ),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn empty_scope_stop_fences_unbound_legacy_only_and_allows_first_new_revision() {
    let mut f = Fixture::new();
    f.work
        .stop_origin(&f.alice, "WinA", "empty-chat", 0, "stop-empty", 10)
        .unwrap();
    let mut unbound = local_turn("unbound", "empty-chat", "turn-1", 1);
    unbound.origin_turn_ref = None;
    unbound.origin_turn_epoch = None;
    assert!(matches!(
        f.work
            .submit_for_device(&f.alice, &unbound, 11, Some("WinA")),
        Err(WorkError::Conflict)
    ));
    let first = epoch(
        &f.work
            .admit_origin_turn(
                &f.alice,
                "WinA",
                "empty-chat",
                "turn-1",
                "admit",
                true,
                1,
                12,
            )
            .unwrap(),
    );
    assert_eq!(first, 1);
    assert!(
        f.work
            .submit_for_device(
                &f.alice,
                &local_turn("new", "empty-chat", "turn-1", first),
                13,
                Some("WinA")
            )
            .is_ok()
    );
}

#[test]
fn project_stop_needs_every_live_assignee_or_manager_and_denial_is_atomic() {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        1,
    )
    .unwrap();
    let root = f.submit("solver", "assigned-to-bob", 0);
    f.work
        .handover(
            &f.alice,
            &root.id,
            &HandoverRequest {
                expected_revision: root.revision,
                new_assignee_id: f.bob.user_id.clone(),
            },
            11,
        )
        .unwrap();
    assert!(matches!(
        f.work
            .stop_project_conversation(&f.alice, "alpha", &root.id, "alice-stop", 12),
        Err(WorkError::Forbidden)
    ));
    assert_ne!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
    let fenced: bool = f
        .work
        .db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_conversation_stop_fences WHERE conversation_id=?1)",
            [&root.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!fenced);
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.admin.user_id,
        Some(ProjectRole::Manager),
        13,
    )
    .unwrap();
    f.work
        .stop_project_conversation(&f.admin, "alpha", &root.id, "manager-stop", 14)
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
}

#[test]
fn origin_stop_across_projects_checks_every_current_grant_before_any_fence() {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "beta",
        &f.alice.user_id,
        Some(ProjectRole::Contributor),
        1,
    )
    .unwrap();
    let epoch = epoch(
        &f.work
            .admit_origin_turn(&f.alice, "WinA", "chat", "turn-1", "admit", true, 1, 10)
            .unwrap(),
    );
    let alpha = f
        .work
        .submit_for_device(
            &f.alice,
            &local_turn("alpha-job", "chat", "turn-1", epoch),
            11,
            Some("WinA"),
        )
        .unwrap();
    let mut beta_request = local_turn("beta-job", "chat", "turn-1", epoch);
    beta_request.project_id = "beta".into();
    let beta = f
        .work
        .submit_for_device(&f.alice, &beta_request, 11, Some("WinA"))
        .unwrap();
    identity::set_project_role(&f.work.db, &f.admin, "beta", &f.alice.user_id, None, 12).unwrap();
    assert!(matches!(
        f.work
            .stop_origin(&f.alice, "WinA", "chat", 1, "stop-all", 13),
        Err(WorkError::Identity(_))
    ));
    assert_eq!(
        f.work.job(&f.alice, &alpha.id).unwrap().state,
        JobState::Queued
    );
    let floor:Option<u64>=f.work.db.query_row("SELECT stop_through_revision FROM shared_origin_scopes WHERE origin_device_id='WinA' AND origin_owner_id=?1 AND origin_session_ref='chat'",[&f.alice.user_id],|r|r.get(0)).unwrap();
    assert_eq!(floor, None);
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "beta",
        &f.alice.user_id,
        Some(ProjectRole::Contributor),
        14,
    )
    .unwrap();
    f.work
        .stop_origin(&f.alice, "WinA", "chat", 1, "stop-all", 15)
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &alpha.id).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        f.work.job(&f.alice, &beta.id).unwrap().state,
        JobState::Cancelled
    );
}
