use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

fn upload(key: &str, name: &str, bytes: &[u8]) -> AssetUpload {
    AssetUpload {
        request_id: key.into(),
        name: name.into(),
        content_base64: STANDARD.encode(bytes),
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}
fn archive(attempt: &Assignment, key: &str, checkpoint: Option<&Value>) -> RunnerAssetUpload {
    let mut archive = json!({"version":1,"job_id":attempt.job.id,"project_id":attempt.job.project_id,"environment_id":attempt.job.environment_id,"transcript":[{"kind":"message","payload":{"text":"canonical answer"}}]});
    if let Some(checkpoint) = checkpoint {
        archive["tables"] =
            json!({"shared_run_checkpoints":[{"checkpoint_json":checkpoint.to_string()}]});
    }
    RunnerAssetUpload {
        generation: attempt.generation,
        kind: "archive".into(),
        base_sha256: None,
        upload: upload(
            key,
            "canonical.json",
            &serde_json::to_vec(&archive).unwrap(),
        ),
    }
}
fn with_input(f: &mut Fixture, key: &str, asset: &str) -> Job {
    f.work
        .submit(
            &f.alice,
            &SubmitJob {
                request_id: key.into(),
                project_id: "alpha".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "parent".into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: key.into(),
                input: json!({"version":2,"prompt":"Use shared input","input_refs":[asset]}),
                descendant_budget: 2,
                start_before_ms: None,
            },
            10,
        )
        .unwrap()
}
fn prune(f: &mut Fixture, before: u64, now: u64) {
    let revision = f.work.management_snapshot(&f.admin).unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    f.work
        .manage(
            &f.admin,
            &ManagementCommand {
                request_id: ulid::Ulid::new().to_string(),
                expected_revision: revision,
                request: ManagementRequest::PruneSharedData { before_ms: before },
            },
            now,
        )
        .unwrap();
}

#[test]
fn revising_latest_stopped_turn_keeps_audit_and_resumes_before_the_edited_turn() {
    let mut f = Fixture::new();
    let first = f.submit("parent", "first-for-revise", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "start-for-revise", ReportOutcome::Started);
    let saved = f
        .work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &archive(&attempt, "archive-for-revise", None),
            12,
        )
        .unwrap();
    f.report(&attempt, "finish-for-revise", finished());
    let first = f.work.job(&f.alice, &first.id).unwrap();
    let second = f
        .work
        .continue_job(
            &f.alice,
            &first.id,
            &ContinueJob {
                request_id: "turn-to-edit".into(),
                expected_revision: first.revision,
                project_participation: None,
                conversation_epoch: None,
                prompt: "before edit".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            22,
        )
        .unwrap();
    let stopped = f.work.cancel(&f.alice, &second.id, 23).unwrap();
    assert_eq!(stopped.state, JobState::Cancelled);
    let catalog = f
        .work
        .list_project_conversations(&f.alice, "alpha")
        .unwrap();
    let row = catalog["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == first.id)
        .unwrap();
    assert_eq!(row["can_revise"], true);
    assert_eq!(row["revision"], 2);
    let replacement = SubmitJob {
        request_id: "turn-edited".into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: Some(first.conversation_id.clone()),
        revises_job_id: Some(stopped.id.clone()),
        expected_revised_revision: Some(stopped.revision),
        environment_id: String::new(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "Edited turn".into(),
        input: json!({"prompt":"after edit"}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    let edited = f.work.submit(&f.alice, &replacement, 24).unwrap();
    assert_eq!(edited.conversation_id, first.id);
    assert_eq!(edited.revises_job_id.as_deref(), Some(stopped.id.as_str()));
    assert_eq!(edited.continued_from.as_deref(), Some(first.id.as_str()));
    let after_append = f
        .work
        .list_project_conversations(&f.alice, "alpha")
        .unwrap();
    let current = after_append["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == first.id)
        .unwrap();
    assert_eq!(current["revision"], 3);
    assert!(matches!(
        f.work
            .rename_project_conversation(&f.alice, "alpha", &first.id, "stale", 2, 25),
        Err(WorkError::Conflict)
    ));
    assert_eq!(
        f.work.submit(&f.alice, &replacement, 25).unwrap().id,
        edited.id
    );
    assert!(matches!(
        f.work.submit(
            &f.alice,
            &SubmitJob {
                request_id: "stale-revise".into(),
                ..replacement.clone()
            },
            26
        ),
        Err(WorkError::Conflict)
    ));
    let history = f
        .work
        .conversation_history(&f.alice, "alpha", &first.id, None, None, 10, 27)
        .unwrap();
    assert_eq!(history.jobs.len(), 3);
    assert_eq!(
        history.jobs[0].job.revises_job_id.as_deref(),
        Some(stopped.id.as_str())
    );
    let running = f.work.claim("WinC", 28).unwrap().unwrap();
    assert_eq!(running.job.id, edited.id);
    assert_eq!(
        f.work
            .runner_archive("WinC", &running.attempt_id)
            .unwrap()
            .unwrap()
            .asset
            .id,
        saved.id
    );
}

#[test]
fn first_turn_revision_has_no_predecessor_and_needs_settled_stop() {
    let mut f = Fixture::new();
    let first = f.submit("parent", "first-stop-revise", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    let cancelling = f.work.cancel(&f.alice, &first.id, 12).unwrap();
    assert_eq!(cancelling.state, JobState::Cancelling);
    let request = SubmitJob {
        request_id: "first-edited".into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: Some(first.id.clone()),
        revises_job_id: Some(first.id.clone()),
        expected_revised_revision: Some(cancelling.revision),
        environment_id: "parent".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "edited".into(),
        input: json!({"prompt":"after edit"}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    assert!(matches!(
        f.work.submit(&f.alice, &request, 13),
        Err(WorkError::Conflict)
    ));
    f.report(
        &attempt,
        "stopped-for-revise",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
    );
    let cancelled = f.work.job(&f.alice, &first.id).unwrap();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        21,
    )
    .unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.bob, &first.id, 21).unwrap()["can_revise"],
        false
    );
    let request = SubmitJob {
        expected_revised_revision: Some(cancelled.revision),
        ..request
    };
    assert!(matches!(
        f.work.submit(&f.bob, &request, 22),
        Err(WorkError::Forbidden)
    ));
    let edited = f.work.submit(&f.alice, &request, 22).unwrap();
    assert_eq!(edited.continued_from, None);
}

#[test]
fn conversation_delete_waits_for_runner_stop_then_hides_canonical_history() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "conversation-to-delete", 0);
    let listed = f
        .work
        .list_project_conversations(&f.alice, "alpha")
        .unwrap();
    assert_eq!(listed["conversations"][0]["id"], root.id);
    assert_eq!(listed["conversations"][0]["can_rename"], true);
    assert!(
        f.work
            .rename_project_conversation(&f.bob, "alpha", &root.id, "denied", 1, 11)
            .is_err()
    );
    let renamed = f
        .work
        .rename_project_conversation(&f.alice, "alpha", &root.id, "新しい題名", 1, 11)
        .unwrap();
    assert_eq!(renamed["title"], "新しい題名");
    assert_eq!(renamed["revision"], 2);
    assert!(matches!(
        f.work
            .rename_project_conversation(&f.alice, "alpha", &root.id, "stale", 1, 12),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        f.work
            .delete_project_conversation(&f.alice, "alpha", &root.id, 1, 12),
        Err(WorkError::Conflict)
    ));
    let running = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&running, "start-to-delete", ReportOutcome::Started);
    let deleting = f
        .work
        .delete_project_conversation(&f.alice, "alpha", &root.id, 2, 21)
        .unwrap();
    assert_eq!(deleting["delete_pending"], true);
    let pending = f
        .work
        .list_project_conversations(&f.alice, "alpha")
        .unwrap();
    assert_eq!(pending["conversations"][0]["delete_pending"], true);
    assert_eq!(pending["conversations"][0]["can_rename"], false);
    assert!(
        f.work
            .rename_project_conversation(&f.alice, "alpha", &root.id, "late rename", 3, 22)
            .is_err()
    );
    f.report(
        &running,
        "stopped-after-delete",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
    );
    let deleted = f
        .work
        .delete_project_conversation(&f.alice, "alpha", &root.id, 2, 23)
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert!(
        f.work
            .list_project_conversations(&f.alice, "alpha")
            .unwrap()["conversations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        f.work.job(&f.alice, &root.id),
        Err(WorkError::NotFound)
    ));
    assert!(matches!(
        f.work
            .conversation_history(&f.alice, "alpha", &root.id, None, None, 10, 24),
        Err(WorkError::NotFound)
    ));
    let state: String = f
        .work
        .db
        .query_row(
            "SELECT state FROM shared_jobs WHERE id=?1",
            [&root.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "cancelled");
}

#[test]
fn conversation_catalog_latest_job_uses_save_order_when_timestamps_tie() {
    let mut f = Fixture::new();
    let first = f.submit("parent", "same-timestamp-first", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "same-timestamp-start", ReportOutcome::Started);
    f.report(&attempt, "same-timestamp-done", finished());
    let latest = "000-same-timestamp-latest";
    f.work.db.execute(
        "INSERT INTO shared_jobs(id,root_id,conversation_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms)
         SELECT ?2,?2,?1,project_id,environment_id,requestor_id,assignee_id,'same-timestamp-receipt','{}','Latest','{}','succeeded',0,1,created_at_ms,updated_at_ms FROM shared_jobs WHERE id=?1",
        rusqlite::params![first.id,latest],
    ).unwrap();
    let row = &f
        .work
        .list_project_conversations(&f.alice, "alpha")
        .unwrap()["conversations"][0];
    assert_eq!(row["latest_job_id"], latest);
    assert_eq!(row["can_revise"], true);
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 30).unwrap()["can_revise"],
        false
    );
}

#[test]
fn continuation_uses_only_the_latest_settled_root_and_never_overlaps_a_turn() {
    let mut f = Fixture::new();
    f.work
        .db
        .execute(
            "UPDATE shared_projects SET overview='original overview' WHERE id='alpha'",
            [],
        )
        .unwrap();
    let first = f.submit("parent", "first-continuation-root", 0);
    let first_attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    assert_eq!(
        first_attempt.project_context.as_ref().unwrap().overview,
        "original overview"
    );
    f.report(
        &first_attempt,
        "first-continuation-start",
        ReportOutcome::Started,
    );
    f.work
        .runner_upload_asset(
            "WinC",
            &first_attempt.attempt_id,
            &archive(&first_attempt, "first-continuation-archive", None),
            12,
        )
        .unwrap();
    f.report(&first_attempt, "first-continuation-done", finished());
    let first = f.work.job(&f.alice, &first.id).unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 20).unwrap()["can_continue"],
        true
    );
    let request = ContinueJob {
        request_id: "second-continuation-root".into(),
        expected_revision: first.revision,
        project_participation: None,
        conversation_epoch: None,
        prompt: "Second turn".into(),
        input_refs: vec![],
        start_before_ms: None,
    };
    f.work
        .db
        .execute(
            "UPDATE shared_projects SET overview='next request overview' WHERE id='alpha'",
            [],
        )
        .unwrap();
    let second = f
        .work
        .continue_job(&f.alice, &first.id, &request, 21)
        .unwrap();
    assert_eq!(
        f.work
            .continue_job(&f.alice, &first.id, &request, 22)
            .unwrap()
            .id,
        second.id
    );
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 22).unwrap()["can_continue"],
        false
    );
    let late = ContinueJob {
        request_id: "parallel-third-root".into(),
        ..request
    };
    assert!(matches!(
        f.work.continue_job(&f.alice, &first.id, &late, 22),
        Err(WorkError::Conflict)
    ));
    let second_attempt = f.work.claim("WinC", 23).unwrap().unwrap();
    assert_eq!(second_attempt.job.id, second.id);
    let next_context = second_attempt.project_context.as_ref().unwrap();
    assert_eq!(next_context.overview, "next request overview");
    assert_eq!(next_context.root_prompt, "");
    assert_eq!(second_attempt.job.input["prompt"], "Second turn");
    f.report(
        &second_attempt,
        "second-continuation-start",
        ReportOutcome::Started,
    );
    assert!(matches!(
        f.work.continue_job(&f.alice, &first.id, &late, 24),
        Err(WorkError::Conflict)
    ));
    f.work
        .runner_upload_asset(
            "WinC",
            &second_attempt.attempt_id,
            &archive(&second_attempt, "second-continuation-archive", None),
            25,
        )
        .unwrap();
    f.report(&second_attempt, "second-continuation-done", finished());
    let second = f.work.job(&f.alice, &second.id).unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 26).unwrap()["can_continue"],
        false
    );
    assert_eq!(
        f.work.job_detail_at(&f.alice, &second.id, 26).unwrap()["can_continue"],
        true
    );
    assert!(matches!(
        f.work.continue_job(&f.alice, &first.id, &late, 27),
        Err(WorkError::Conflict)
    ));
    let third = f
        .work
        .continue_job(
            &f.alice,
            &second.id,
            &ContinueJob {
                request_id: "third-continuation-root".into(),
                expected_revision: second.revision,
                project_participation: None,
                conversation_epoch: None,
                prompt: "Third turn".into(),
                input_refs: vec![],
                start_before_ms: None,
            },
            27,
        )
        .unwrap();
    assert_eq!(third.conversation_id, first.id);
}

#[test]
fn delete_pending_conversation_cannot_accept_continuation_while_old_work_stops() {
    let mut f = Fixture::new();
    let first = f.submit("parent", "delete-pending-root", 0);
    let first_attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(
        &first_attempt,
        "delete-pending-start",
        ReportOutcome::Started,
    );
    f.work
        .runner_upload_asset(
            "WinC",
            &first_attempt.attempt_id,
            &archive(&first_attempt, "delete-pending-archive", None),
            12,
        )
        .unwrap();
    f.report(
        &first_attempt,
        "delete-pending-retain",
        ReportOutcome::RetainService {
            service_id: "delete-pending-service".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&first_attempt, "delete-pending-finish", finished());
    let first = f.work.job(&f.alice, &first.id).unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 21).unwrap()["can_continue"],
        true
    );
    let continuation = ContinueJob {
        request_id: "delete-pending-second".into(),
        expected_revision: first.revision,
        project_participation: None,
        conversation_epoch: None,
        prompt: "Second turn".into(),
        input_refs: vec![],
        start_before_ms: None,
    };
    let second = f
        .work
        .continue_job(&f.alice, &first.id, &continuation, 22)
        .unwrap();
    let second_attempt = f.work.claim("WinC", 23).unwrap().unwrap();
    assert_eq!(second_attempt.job.id, second.id);
    f.report(
        &second_attempt,
        "delete-pending-second-start",
        ReportOutcome::Started,
    );
    let deleted = f
        .work
        .delete_project_conversation(&f.alice, "alpha", &first.id, 2, 24)
        .unwrap();
    assert_eq!(deleted["delete_pending"], true);
    assert_eq!(
        f.work.job_detail_at(&f.alice, &first.id, 25).unwrap()["can_continue"],
        false
    );
    assert!(
        f.work
            .attempt("WinC", &second_attempt.attempt_id)
            .unwrap()
            .assignment
            .stop_requested
    );
    assert_eq!(
        f.work
            .continue_job(&f.alice, &first.id, &continuation, 25)
            .unwrap()
            .id,
        second.id
    );
    let late = ContinueJob {
        request_id: "late-after-delete".into(),
        ..continuation
    };
    assert!(matches!(
        f.work.continue_job(&f.alice, &first.id, &late, 25),
        Err(WorkError::Conflict)
    ));
    f.report(
        &second_attempt,
        "delete-pending-second-stopped",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
    );
    assert_eq!(
        f.work
            .list_project_conversations(&f.alice, "alpha")
            .unwrap()["conversations"][0]["delete_pending"],
        true
    );
    f.work
        .report(
            "WinC",
            &Report {
                event_id: "delete-pending-service-stopped".into(),
                attempt_id: first_attempt.attempt_id,
                generation: first_attempt.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "delete-pending-service".into(),
                },
            },
            26,
        )
        .unwrap();
    assert!(
        f.work
            .list_project_conversations(&f.alice, "alpha")
            .unwrap()["conversations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn attached_revision() -> (Fixture, Asset, SubmitJob) {
    let mut f = Fixture::new();
    let asset = f
        .work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("attached-revise-input", "source.txt", b"data"),
            5,
        )
        .unwrap();
    let root = with_input(&mut f, "attached-root", &asset.id);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "attached-revise-start", ReportOutcome::Started);
    f.report(&attempt, "attached-revise-finish", finished());
    let terminal = f.work.job(&f.alice, &root.id).unwrap();
    assert_eq!(terminal.state, JobState::Succeeded);
    let revise = SubmitJob {
        request_id: "preserve-attachment".into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: Some(root.id.clone()),
        revises_job_id: Some(root.id.clone()),
        expected_revised_revision: Some(terminal.revision),
        environment_id: "parent".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "edited".into(),
        input: json!({"version":2,"prompt":"edited with the same file","input_refs":[asset.id]}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    (f, asset, revise)
}

#[test]
fn revising_attached_latest_turn_preserves_inputs_and_binds_new_attempt() {
    let (mut f, asset, revise) = attached_revision();
    let root = revise.revises_job_id.as_deref().unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.alice, root, 21).unwrap()["can_revise"],
        true
    );
    assert_eq!(
        f.work
            .list_project_conversations(&f.alice, "alpha")
            .unwrap()["conversations"][0]["can_revise"],
        true
    );
    let edited = f.work.submit(&f.alice, &revise, 22).unwrap();
    assert_eq!(edited.input["input_refs"], json!([asset.id]));
    assert_eq!(edited.revises_job_id.as_deref(), Some(root));
    assert_eq!(
        f.work.job_assets(&f.alice, &edited.id).unwrap(),
        vec![asset.clone()]
    );
    assert_eq!(f.work.submit(&f.alice, &revise, 23).unwrap().id, edited.id);
    f.reopen();
    let attempt = f.work.claim("WinC", 24).unwrap().unwrap();
    assert_eq!(attempt.job.id, edited.id);
    let input = f
        .work
        .runner_asset("WinC", &attempt.attempt_id, &asset.id)
        .unwrap();
    assert_eq!(input.asset, asset);
    assert_eq!(STANDARD.decode(input.content_base64).unwrap(), b"data");
    assert!(
        f.work
            .runner_asset("WinE", &attempt.attempt_id, &asset.id)
            .is_err()
    );
    // The old job is settled, but the edited job must keep its shared input alive.
    prune(&mut f, 30, 40);
    assert!(
        f.work
            .runner_asset("WinC", &attempt.attempt_id, &asset.id)
            .is_ok()
    );
}

#[test]
fn revising_attached_latest_turn_rejects_changed_or_missing_inputs() {
    let (mut f, asset, revise) = attached_revision();
    let other = f
        .work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("other-edit-input", "other.txt", b"other"),
            21,
        )
        .unwrap();
    for input in [
        json!({"prompt":"drop refs"}),
        json!({"version":2,"prompt":"empty refs","input_refs":[]}),
        json!({"version":2,"prompt":"replace ref","input_refs":[other.id]}),
        json!({"version":2,"prompt":"add ref","input_refs":[asset.id,other.id]}),
    ] {
        assert!(matches!(
            f.work.submit(
                &f.alice,
                &SubmitJob {
                    input,
                    ..revise.clone()
                },
                22
            ),
            Err(WorkError::Conflict)
        ));
    }
    assert_eq!(
        f.work
            .conversation_history(
                &f.alice,
                "alpha",
                revise.conversation_id.as_deref().unwrap(),
                None,
                None,
                10,
                23
            )
            .unwrap()
            .jobs
            .len(),
        1
    );
    assert!(f.work.submit(&f.alice, &revise, 24).is_ok());
}

#[test]
fn revising_attached_latest_turn_revalidates_retention_and_project_authority() {
    let (mut f, asset, revise) = attached_revision();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        21,
    )
    .unwrap();
    assert!(f.work.submit(&f.alice, &revise, 22).is_err());
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Contributor),
        23,
    )
    .unwrap();
    prune(&mut f, 30, 40);
    assert!(matches!(
        f.work.asset(&f.alice, &asset.id),
        Err(WorkError::NotFound)
    ));
    assert!(matches!(
        f.work.submit(&f.alice, &revise, 41),
        Err(WorkError::NotFound)
    ));
    assert_eq!(
        f.work
            .conversation_history(
                &f.alice,
                "alpha",
                revise.conversation_id.as_deref().unwrap(),
                None,
                None,
                10,
                42
            )
            .unwrap()
            .jobs
            .len(),
        1
    );
}

#[test]
fn revising_unattached_latest_turn_rejects_new_inputs_and_accepts_empty_legacy_form() {
    let (mut f, asset, mut revise) = attached_revision();
    let plain = f.submit("parent", "plain-root", 0);
    let stopped = f.work.cancel(&f.alice, &plain.id, 21).unwrap();
    revise.conversation_id = Some(plain.id.clone());
    revise.revises_job_id = Some(plain.id.clone());
    revise.expected_revised_revision = Some(stopped.revision);
    assert_eq!(revise.input["input_refs"], json!([asset.id]));
    assert!(matches!(
        f.work.submit(&f.alice, &revise, 22),
        Err(WorkError::Conflict)
    ));
    revise.input["input_refs"] = json!([]);
    assert!(f.work.submit(&f.alice, &revise, 23).is_ok());
}

#[test]
fn data_transcript_keeps_oversized_single_items_and_pages_without_loss() {
    let mut f = Fixture::new();
    let job = f.submit("parent", "large-transcript", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let content = "x".repeat(4 * 1024 * 1024);
    let value = json!({"version":1,"job_id":job.id,"project_id":job.project_id,"environment_id":job.environment_id,
        "transcript":[{"kind":"tool_output","payload":{"text":content}},{"kind":"assistant_message","payload":{"text":"last"}}]});
    let request = RunnerAssetUpload {
        generation: attempt.generation,
        kind: "archive".into(),
        base_sha256: None,
        upload: upload(
            "large-history",
            "history.json",
            &serde_json::to_vec(&value).unwrap(),
        ),
    };
    f.work
        .runner_upload_asset("WinC", &attempt.attempt_id, &request, 21)
        .unwrap();
    let first = f.work.transcript(&f.alice, &job.id, 0, 100).unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].payload["text"], content);
    assert_eq!(first.next_after, Some(1));
    let next = f.work.transcript(&f.alice, &job.id, 1, 100).unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].payload["text"], "last");
    assert_eq!(next.next_after, None);
    assert!(f.work.transcript(&f.bob, &job.id, 0, 100).is_err());
}

#[test]
fn data_upload_retry_and_current_project_download_authorization_are_exact() {
    let mut f = Fixture::new();
    let request = upload("input-one", "data.csv", b"x,y\n1,2\n");
    let input = f.work.upload_input(&f.alice, "alpha", &request, 5).unwrap();
    assert_eq!(
        f.work.upload_input(&f.alice, "alpha", &request, 6).unwrap(),
        input
    );
    assert!(matches!(
        f.work.upload_input(
            &f.alice,
            "alpha",
            &upload("input-one", "data.csv", b"changed"),
            7
        ),
        Err(WorkError::Conflict)
    ));
    assert!(f.work.asset(&f.bob, &input.id).is_err());
    assert!(f.work.asset(&f.admin, &input.id).is_err());
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Reader),
        8,
    )
    .unwrap();
    assert_eq!(
        STANDARD
            .decode(f.work.asset(&f.bob, &input.id).unwrap().content_base64)
            .unwrap(),
        b"x,y\n1,2\n"
    );
    assert!(
        f.work
            .upload_input(&f.bob, "alpha", &upload("reader-upload", "x.txt", b"no"), 9)
            .is_err()
    );
    let beta = f
        .work
        .upload_input(&f.bob, "beta", &upload("beta", "private.txt", b"beta"), 9)
        .unwrap();
    assert!(
        f.work
            .submit(
                &f.alice,
                &SubmitJob {
                    request_id: "foreign-ref".into(),
                    project_id: "alpha".into(),
                    project_participation: None,
                    conversation_id: None,
                    revises_job_id: None,
                    expected_revised_revision: None,
                    environment_id: "solver".into(),
                    origin_session_ref: None,
                    origin_turn_ref: None,
                    origin_turn_epoch: None,
                    title: "no".into(),
                    input: json!({"version":2,"prompt":"no","input_refs":[beta.id]}),
                    descendant_budget: 0,
                    start_before_ms: None,
                },
                10
            )
            .is_err()
    );
}

#[test]
fn data_input_references_bind_parent_and_child_without_broad_runner_access() {
    let mut f = Fixture::new();
    let input = f
        .work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("linked", "data.csv", b"table"),
            5,
        )
        .unwrap();
    let unrelated = f
        .work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("other", "other.csv", b"private"),
            5,
        )
        .unwrap();
    let root = with_input(&mut f, "parent-input", &input.id);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    assert_eq!(
        f.work
            .runner_asset("WinC", &parent.attempt_id, &input.id)
            .unwrap()
            .asset
            .id,
        input.id
    );
    assert!(
        f.work
            .runner_asset("WinC", &parent.attempt_id, &unrelated.id)
            .is_err()
    );
    assert!(
        f.work
            .runner_asset("WinE", &parent.attempt_id, &input.id)
            .is_err()
    );
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(
        &parent,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "Compute".into(),
            child_input: json!({"version":2,"prompt":"compute","input_refs":[input.id]}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    assert_eq!(child.job.parent_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(
        f.work
            .runner_asset("WinE", &child.attempt_id, &input.id)
            .unwrap()
            .asset
            .id,
        input.id
    );
    assert_eq!(f.work.job_assets(&f.alice, &child.job.id).unwrap().len(), 1);
    f.reopen();
    assert!(
        f.work
            .runner_asset("WinE", &child.attempt_id, &input.id)
            .is_ok()
    );
}

#[test]
fn runner_can_stage_exact_child_input_from_its_running_attempt() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "runner-staged-input", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    let staged = RunnerAssetUpload {
        generation: parent.generation,
        kind: "input".into(),
        base_sha256: None,
        upload: upload("staged-source", "src/app.py", b"print('ok')\n"),
    };
    assert!(matches!(
        f.work
            .runner_upload_asset("WinC", &parent.attempt_id, &staged, 12),
        Err(WorkError::Conflict)
    ));
    f.report(&parent, "staged-start", ReportOutcome::Started);
    let saved = f
        .work
        .runner_upload_asset("WinC", &parent.attempt_id, &staged, 13)
        .unwrap();
    assert_eq!(saved.kind, "input");
    assert_eq!(saved.job_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(
        f.work
            .runner_upload_asset("WinC", &parent.attempt_id, &staged, 14)
            .unwrap(),
        saved
    );
    assert!(
        f.work
            .runner_upload_asset("WinE", &parent.attempt_id, &staged, 14)
            .is_err()
    );
    assert!(matches!(
        f.work.runner_upload_asset(
            "WinC",
            &parent.attempt_id,
            &RunnerAssetUpload {
                generation: parent.generation + 1,
                ..staged.clone()
            },
            14
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        f.work.runner_upload_asset(
            "WinC",
            &parent.attempt_id,
            &RunnerAssetUpload {
                base_sha256: Some("0".repeat(64)),
                ..staged.clone()
            },
            14
        ),
        Err(WorkError::Invalid)
    ));
    f.report(
        &parent,
        "staged-yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "Use staged file".into(),
            child_input: json!({"version":2,"prompt":"Use file","input_refs":[saved.id]}),
            resources_released: true,
        },
    );
    assert_eq!(
        f.work
            .runner_upload_asset("WinC", &parent.attempt_id, &staged, 20)
            .unwrap(),
        saved
    );
    assert!(matches!(
        f.work.runner_upload_asset(
            "WinC",
            &parent.attempt_id,
            &RunnerAssetUpload {
                upload: upload("late-staged", "late.txt", b"late"),
                ..staged.clone()
            },
            20
        ),
        Err(WorkError::Conflict)
    ));
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    assert_eq!(child.job.parent_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(
        f.work
            .runner_asset("WinE", &child.attempt_id, &saved.id)
            .unwrap()
            .asset
            .id,
        saved.id
    );
    prune(&mut f, 30, 31);
    assert!(f.work.asset(&f.alice, &saved.id).is_ok());
}

#[test]
fn data_runner_new_downloads_recheck_current_person_and_cancellation() {
    for revoke in [false, true] {
        let mut f = Fixture::new();
        let input = f
            .work
            .upload_input(
                &f.alice,
                "alpha",
                &upload("private-input", "input.txt", b"private"),
                5,
            )
            .unwrap();
        let root = with_input(&mut f, "revoked-download", &input.id);
        let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
        f.report(&attempt, "start", ReportOutcome::Started);
        f.work
            .runner_upload_asset(
                "WinC",
                &attempt.attempt_id,
                &archive(&attempt, "private-archive", None),
                21,
            )
            .unwrap();
        assert!(
            f.work
                .runner_asset("WinC", &attempt.attempt_id, &input.id)
                .is_ok()
        );
        assert!(
            f.work
                .runner_archive("WinC", &attempt.attempt_id)
                .unwrap()
                .is_some()
        );
        if revoke {
            identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 22)
                .unwrap();
        } else {
            f.work.cancel(&f.alice, &root.id, 22).unwrap();
        }
        assert!(
            f.work
                .runner_asset("WinC", &attempt.attempt_id, &input.id)
                .is_err()
        );
        assert!(f.work.runner_archive("WinC", &attempt.attempt_id).is_err());
    }
}

#[test]
fn resumed_parent_runner_reads_only_its_immediate_child_artifacts() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "child-artifact-scope", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(
        &parent,
        "child-artifact-parent-start",
        ReportOutcome::Started,
    );
    let parent_artifact = f
        .work
        .runner_upload_asset(
            "WinC",
            &parent.attempt_id,
            &RunnerAssetUpload {
                generation: parent.generation,
                kind: "artifact".into(),
                base_sha256: None,
                upload: upload("parent-artifact", "parent.txt", b"parent"),
            },
            12,
        )
        .unwrap();
    f.report(&parent, "child-artifact-yield", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&child, "child-artifact-child-start", ReportOutcome::Started);
    let artifact = f
        .work
        .runner_upload_asset(
            "WinE",
            &child.attempt_id,
            &RunnerAssetUpload {
                generation: child.generation,
                kind: "artifact".into(),
                base_sha256: None,
                upload: upload("child-artifact", "result.txt", b"answer 42"),
            },
            22,
        )
        .unwrap();
    let child_archive = f
        .work
        .runner_upload_asset(
            "WinE",
            &child.attempt_id,
            &archive(&child, "child-artifact-archive", None),
            23,
        )
        .unwrap();
    f.report(&child, "child-artifact-child-done", finished());
    let resumed = f.work.claim("WinC", 24).unwrap().unwrap();
    assert_eq!(resumed.job.id, root.id);
    assert_eq!(resumed.generation, 2);
    let items = f
        .work
        .runner_child_artifacts(
            "WinC",
            &resumed.attempt_id,
            resumed.generation,
            &child.job.id,
        )
        .unwrap();
    assert_eq!(items, vec![artifact.clone()]);
    let downloaded = f
        .work
        .runner_child_artifact(
            "WinC",
            &resumed.attempt_id,
            resumed.generation,
            &child.job.id,
            &artifact.id,
        )
        .unwrap();
    assert_eq!(
        STANDARD.decode(downloaded.content_base64).unwrap(),
        b"answer 42"
    );
    assert!(matches!(
        f.work.runner_child_artifacts(
            "WinC",
            &resumed.attempt_id,
            resumed.generation + 1,
            &child.job.id
        ),
        Err(WorkError::Conflict)
    ));
    assert!(
        f.work
            .runner_child_artifacts(
                "WinE",
                &resumed.attempt_id,
                resumed.generation,
                &child.job.id
            )
            .is_err()
    );
    assert!(
        f.work
            .runner_child_artifacts("WinC", &resumed.attempt_id, resumed.generation, &root.id)
            .is_err()
    );
    assert!(
        f.work
            .runner_child_artifact(
                "WinC",
                &resumed.attempt_id,
                resumed.generation,
                &child.job.id,
                &parent_artifact.id
            )
            .is_err()
    );
    assert!(
        f.work
            .runner_child_artifact(
                "WinC",
                &resumed.attempt_id,
                resumed.generation,
                &child.job.id,
                &child_archive.id
            )
            .is_err()
    );
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 25).unwrap();
    assert!(
        f.work
            .runner_child_artifacts(
                "WinC",
                &resumed.attempt_id,
                resumed.generation,
                &child.job.id
            )
            .is_err()
    );
}

#[test]
fn data_closed_attempt_replays_received_archive_but_cannot_upload_new_or_changed_effects() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "archive-retry", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let request = archive(&attempt, "canonical-one", None);
    let stored = f
        .work
        .runner_upload_asset("WinE", &attempt.attempt_id, &request, 21)
        .unwrap();
    f.report(&attempt, "done", finished());
    f.reopen();
    assert_eq!(
        f.work
            .runner_upload_asset("WinE", &attempt.attempt_id, &request, 30)
            .unwrap(),
        stored
    );
    assert!(
        f.work
            .runner_upload_asset(
                "WinE",
                &attempt.attempt_id,
                &archive(&attempt, "new-after-terminal", None),
                30
            )
            .is_err()
    );
    let mut changed = request.clone();
    changed.upload = upload("canonical-one", "canonical.json", b"{}");
    assert!(
        f.work
            .runner_upload_asset("WinE", &attempt.attempt_id, &changed, 30)
            .is_err()
    );
    assert!(
        f.work
            .runner_upload_asset("WinC", &attempt.attempt_id, &request, 30)
            .is_err()
    );
    assert!(f.work.asset(&f.alice, &stored.id).is_err());
    let page = f.work.transcript(&f.alice, &job.id, 0, 1).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].payload["text"], "canonical answer");
    assert!(f.work.transcript(&f.bob, &job.id, 0, 1).is_err());
}

#[test]
fn data_portable_checkpoint_must_match_durable_archive_before_capacity_is_returned() {
    let mut f = Fixture::new();
    f.submit("parent", "portable-checkpoint", 1);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let checkpoint = json!({"version":2,"job_id":attempt.job.id,"receipt":"exact"});
    let report = Report {
        event_id: "portable-yield".into(),
        attempt_id: attempt.attempt_id.clone(),
        generation: attempt.generation,
        outcome: ReportOutcome::YieldToChild {
            checkpoint: checkpoint.clone(),
            child_environment_id: "solver".into(),
            child_title: "child".into(),
            child_input: json!({"version":1,"prompt":"compute"}),
            resources_released: true,
        },
    };
    assert!(f.work.report("WinC", &report, 21).is_err());
    assert_eq!(f.work.assignments("WinC").unwrap().len(), 1);
    f.work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &archive(
                &attempt,
                "wrong-checkpoint",
                Some(&json!({"version":2,"receipt":"wrong"})),
            ),
            22,
        )
        .unwrap();
    assert!(f.work.report("WinC", &report, 23).is_err());
    f.work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &archive(&attempt, "right-checkpoint", Some(&checkpoint)),
            24,
        )
        .unwrap();
    let accepted = f.work.report("WinC", &report, 25).unwrap();
    assert_eq!(accepted.state, JobState::WaitingChild);
    assert!(f.work.assignments("WinC").unwrap().is_empty());
    assert_eq!(
        f.work
            .report("WinC", &report, 26)
            .unwrap()
            .awaiting_child_id,
        accepted.awaiting_child_id
    );
}

#[test]
fn data_retention_preserves_active_inputs_and_continuation_archive_then_rejects_purged_reuse() {
    let mut f = Fixture::new();
    let input_request = upload("keep-input", "input.txt", b"input");
    let input = f
        .work
        .upload_input(&f.alice, "alpha", &input_request, 5)
        .unwrap();
    let discard = f
        .work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("old-draft", "discard.txt", b"discard"),
            5,
        )
        .unwrap();
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "continue-data".into(),
                project_id: "alpha".into(),
                project_participation: Some(1),
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "parent".into(),
                origin_session_ref: Some("data-session".into()),
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "continue-data".into(),
                input: json!({"version":2,"prompt":"Use shared input","input_refs":[input.id]}),
                descendant_budget: 2,
                start_before_ms: None,
            },
            10,
            Some("WinA"),
        )
        .unwrap();
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let saved = f
        .work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &archive(&attempt, "continue-archive", None),
            21,
        )
        .unwrap();
    assert_eq!(
        f.work.preview_retention(&f.admin, 30).unwrap()["asset_count"],
        1
    );
    prune(&mut f, 30, 40);
    assert!(f.work.asset(&f.alice, &discard.id).is_err());
    assert!(f.work.asset(&f.alice, &input.id).is_ok());
    f.report(&attempt, "done", finished());
    let terminal = f.work.job(&f.alice, &root.id).unwrap();
    // A finished legacy conversation must not pass its old Runner contract to a new turn.
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability='multi_device_session_v1' WHERE id=?1",
            [&root.id],
        )
        .unwrap();
    let request = ContinueJob {
        request_id: "next-root".into(),
        expected_revision: terminal.revision,
        project_participation: None,
        conversation_epoch: None,
        prompt: "Continue from this conversation".into(),
        input_refs: vec![input.id.clone()],
        start_before_ms: None,
    };
    let invalid_deadline = ContinueJob {
        request_id: "expired-continuation".into(),
        start_before_ms: Some(41),
        ..request.clone()
    };
    assert!(matches!(
        f.work
            .continue_job_for_device(&f.alice, Some("WinA"), &root.id, &invalid_deadline, 41),
        Err(WorkError::StartDeadline)
    ));
    let next = f
        .work
        .continue_job_for_device(&f.alice, Some("WinA"), &root.id, &request, 41)
        .unwrap();
    assert_ne!(next.id, root.id);
    assert_eq!(next.continued_from.as_deref(), Some(root.id.as_str()));
    assert_eq!(next.origin_session_ref.as_deref(), Some("data-session"));
    let continuation_device: String = f
        .work
        .db
        .query_row(
            "SELECT origin_device_id FROM shared_jobs WHERE id=?1",
            [&next.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(continuation_device, "WinA");
    assert_eq!(next.quota_owner_id, root.quota_owner_id);
    assert_eq!(next.start_before_ms, None);
    assert_eq!(
        f.work
            .continue_job_for_device(&f.alice, Some("WinA"), &root.id, &request, 42)
            .unwrap()
            .id,
        next.id
    );
    assert_eq!(
        f.work
            .continue_job_for_device(
                &f.alice,
                Some("WinA"),
                &root.id,
                &request,
                73 * 60 * 60 * 1000
            )
            .unwrap()
            .start_before_ms,
        next.start_before_ms
    );
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().revision,
        terminal.revision
    );
    assert_eq!(
        f.work.preview_retention(&f.admin, 50).unwrap()["asset_count"],
        0
    );
    prune(&mut f, 50, 60);
    assert!(
        f.work
            .claim_for_capability("WinC", None, Some("multi_device_session_v1"), 61)
            .unwrap()
            .is_none()
    );
    let next_attempt = f.work.claim("WinC", 61).unwrap().unwrap();
    assert_eq!(
        next_attempt.required_runner_capabilities,
        [store::MULTI_DEVICE_SESSION_CAPABILITY]
    );
    assert_eq!(
        f.work
            .runner_archive("WinC", &next_attempt.attempt_id)
            .unwrap()
            .unwrap()
            .asset
            .id,
        saved.id
    );
    f.report(&next_attempt, "start", ReportOutcome::Started);
    f.report(&next_attempt, "done", finished());
    prune(&mut f, 70, 80);
    assert!(f.work.asset(&f.alice, &input.id).is_err());
    assert!(
        f.work
            .transcript(&f.alice, &root.id, 0, 100)
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        f.work
            .upload_input(&f.alice, "alpha", &input_request, 81)
            .unwrap()
            .id,
        input.id
    );
    assert!(
        f.work
            .submit(
                &f.alice,
                &SubmitJob {
                    request_id: "purged-input-new-job".into(),
                    project_id: "alpha".into(),
                    project_participation: None,
                    conversation_id: None,
                    revises_job_id: None,
                    expected_revised_revision: None,
                    environment_id: "parent".into(),
                    origin_session_ref: None,
                    origin_turn_ref: None,
                    origin_turn_epoch: None,
                    title: "must refuse".into(),
                    input: json!({"version":2,"prompt":"no","input_refs":[input.id]}),
                    descendant_budget: 0,
                    start_before_ms: None,
                },
                82
            )
            .is_err()
    );
    let mut repeat = request;
    repeat.request_id = "purged-archive-new-job".into();
    assert!(
        f.work
            .continue_job(&f.alice, &root.id, &repeat, 82)
            .is_err()
    );
    assert_eq!(
        f.work.management_snapshot(&f.admin).unwrap()["storage"]["purged_asset_count"],
        3
    );
}

#[test]
fn data_files_live_outside_database_and_reject_corruption_without_overwrite() {
    let mut f = Fixture::new();
    let bytes = [0, 255, 1, 128, 13, 10];
    let request = upload("binary", "result.bin", &bytes);
    let asset = f.work.upload_input(&f.alice, "alpha", &request, 5).unwrap();
    let root = super::super::blob_store::assets_directory(&f.dir.path().join("shared.sqlite3"));
    assert_eq!(std::fs::read(root.join(&asset.sha256)).unwrap(), bytes);
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT length(content) FROM shared_assets WHERE id=?1",
                [&asset.id],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        super::super::blob_store::retained_inventory(&f.work.db).unwrap(),
        vec![(asset.sha256.clone(), bytes.len() as u64)]
    );
    f.reopen();
    assert_eq!(
        STANDARD
            .decode(f.work.asset(&f.alice, &asset.id).unwrap().content_base64)
            .unwrap(),
        bytes
    );
    std::fs::write(root.join(&asset.sha256), b"altered").unwrap();
    assert!(f.work.asset(&f.alice, &asset.id).is_err());
    assert!(
        f.work
            .upload_input(
                &f.alice,
                "alpha",
                &upload("new-id", "another.bin", &bytes),
                6
            )
            .is_err()
    );
    assert_eq!(std::fs::read(root.join(&asset.sha256)).unwrap(), b"altered");
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_assets", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn data_v8_blob_migrates_only_after_verified_file_persistence_and_failed_migration_keeps_blob() {
    for corrupt in [false, true] {
        let mut f = Fixture::new();
        let bytes = b"legacy byte contents";
        let asset = f
            .work
            .upload_input(&f.alice, "alpha", &upload("old", "old.bin", bytes), 5)
            .unwrap();
        let database = f.dir.path().join("shared.sqlite3");
        let file = super::super::blob_store::assets_directory(&database).join(&asset.sha256);
        // Rebuild the old-format content and marker, retaining the exact current identities.
        f.work
            .db
            .execute(
                "UPDATE shared_assets SET content=?2 WHERE id=?1",
                rusqlite::params![asset.id, bytes.as_slice()],
            )
            .unwrap();
        f.remove_post_v10_fixture_schema();
        f.work
            .db
            .execute_batch("DROP INDEX shared_jobs_start_deadline; ALTER TABLE shared_jobs DROP COLUMN start_before_ms; DROP INDEX shared_assets_retained_hash; PRAGMA user_version=8;")
            .unwrap();
        if corrupt {
            std::fs::write(&file, b"different existing file").unwrap();
        } else {
            std::fs::remove_file(&file).unwrap();
        }
        let opened = SharedWork::open(&database);
        if corrupt {
            assert!(opened.is_err());
            assert_eq!(
                f.work
                    .db
                    .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                8
            );
            assert_eq!(
                f.work
                    .db
                    .query_row(
                        "SELECT content FROM shared_assets WHERE id=?1",
                        [&asset.id],
                        |r| r.get::<_, Vec<u8>>(0)
                    )
                    .unwrap(),
                bytes
            );
            assert_eq!(std::fs::read(file).unwrap(), b"different existing file");
        } else {
            f.work = opened.unwrap();
            assert_eq!(
                f.work
                    .db
                    .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                26
            );
            assert_eq!(
                f.work
                    .db
                    .query_row(
                        "SELECT length(content) FROM shared_assets WHERE id=?1",
                        [&asset.id],
                        |r| r.get::<_, u64>(0)
                    )
                    .unwrap(),
                0
            );
            assert_eq!(
                STANDARD
                    .decode(f.work.asset(&f.alice, &asset.id).unwrap().content_base64)
                    .unwrap(),
                bytes
            );
        }
    }
}

#[test]
fn data_prune_preserves_other_project_hash_reference_then_collects_after_last_tombstone() {
    let mut f = Fixture::new();
    let bytes = b"same contents";
    let request = upload("first", "shared.bin", bytes);
    let first = f.work.upload_input(&f.alice, "alpha", &request, 5).unwrap();
    let other = f
        .work
        .upload_input(&f.bob, "beta", &upload("second", "other.bin", bytes), 20)
        .unwrap();
    let path = super::super::blob_store::assets_directory(&f.dir.path().join("shared.sqlite3"))
        .join(&first.sha256);
    assert_eq!(first.sha256, other.sha256);
    assert_eq!(
        f.work.management_snapshot(&f.admin).unwrap()["storage"]["retained_bytes"],
        bytes.len() * 2
    );
    prune(&mut f, 10, 30);
    assert!(f.work.asset(&f.alice, &first.id).is_err());
    assert_eq!(
        STANDARD
            .decode(f.work.asset(&f.bob, &other.id).unwrap().content_base64)
            .unwrap(),
        bytes
    );
    assert!(path.exists());
    prune(&mut f, 25, 40);
    assert!(!path.exists());
    assert_eq!(
        f.work
            .upload_input(&f.alice, "alpha", &request, 41)
            .unwrap()
            .id,
        first.id
    );
    assert!(
        !path.exists(),
        "a replayed purged receipt must not recreate its content"
    );
}

#[test]
fn data_orphan_collection_is_bounded_and_preserves_committed_assets() {
    let mut f = Fixture::new();
    let saved = f
        .work
        .upload_input(&f.alice, "alpha", &upload("kept", "kept.bin", b"keep"), 5)
        .unwrap();
    let root = super::super::blob_store::assets_directory(&f.dir.path().join("shared.sqlite3"));
    for index in 0..129 {
        std::fs::write(
            root.join(format!(".pending-{index:04}")),
            b"interrupted write",
        )
        .unwrap();
    }
    super::super::blob_store::gc(&f.work.db).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    assert!(root.join(&saved.sha256).exists());
    super::super::blob_store::gc(&f.work.db).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    // A crash after file sync but before the reference commits creates only an orphan.
    let uncommitted = b"uncommitted bytes";
    let hash = format!("{:x}", Sha256::digest(uncommitted));
    {
        let transaction = f.work.db.transaction().unwrap();
        super::super::blob_store::persist(&transaction, &hash, uncommitted).unwrap();
    }
    assert!(root.join(&hash).exists());
    f.work
        .upload_input(
            &f.alice,
            "alpha",
            &upload("later", "later.bin", b"later"),
            6,
        )
        .unwrap();
    assert!(!root.join(&hash).exists());
    assert!(f.work.asset(&f.alice, &saved.id).is_ok());
}

#[test]
fn data_managed_directory_cannot_be_redirected_to_external_files() {
    let mut f = Fixture::new();
    let database = f.dir.path().join("shared.sqlite3");
    let root = super::super::blob_store::assets_directory(&database);
    let external = f.dir.path().join("external");
    std::fs::create_dir(&external).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("cmd.exe")
            .creation_flags(0x08000000)
            .args(["/C", "mklink", "/J"])
            .arg(&root)
            .arg(&external)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "create isolated junction fixture");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &root).unwrap();
    assert!(
        f.work
            .upload_input(
                &f.alice,
                "alpha",
                &upload("escape", "outside.bin", b"secret"),
                5
            )
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&external).unwrap().count(), 0);
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_assets", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}

#[cfg(windows)]
#[test]
fn data_verified_blob_pins_windows_parent_and_exact_source_during_read() {
    use std::io::Read;
    let mut f = Fixture::new();
    let bytes = b"held source";
    let asset = f
        .work
        .upload_input(&f.alice, "alpha", &upload("held", "source.bin", bytes), 5)
        .unwrap();
    let database = f.dir.path().join("shared.sqlite3");
    let root = super::super::blob_store::assets_directory(&database);
    let replacement = f.dir.path().join("renamed-assets");
    assert!(root.is_absolute() && root.parent() == Some(f.dir.path()));
    assert!(replacement.is_absolute() && replacement.parent() == Some(f.dir.path()));
    let mut held =
        super::super::blob_store::open_verified(&database, &asset.sha256, asset.byte_length)
            .unwrap();
    assert!(
        std::fs::rename(&root, &replacement).is_err(),
        "held parent must not be replaced while a blob is read"
    );
    assert!(
        std::fs::write(root.join(&asset.sha256), b"changed").is_err(),
        "held content must not accept another writer"
    );
    let mut actual = Vec::new();
    held.file_mut().read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
}

#[test]
fn data_restore_cancels_old_queued_roots_and_holds_active_descendants_without_replay() {
    let mut f = Fixture::new();
    let completed = f.submit("parent", "completed-before-backup", 0);
    let completed_attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(
        &completed_attempt,
        "completed-start",
        ReportOutcome::Started,
    );
    f.report(&completed_attempt, "completed-done", finished());
    let parent = f.submit("parent", "parent-before-backup", 1);
    let parent_attempt = f.work.claim("WinC", 31).unwrap().unwrap();
    f.report(&parent_attempt, "parent-start", ReportOutcome::Started);
    f.report(
        &parent_attempt,
        "parent-yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "active descendant".into(),
            child_input: json!({"version":1,"prompt":"work"}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinE", 41).unwrap().unwrap();
    f.report(&child, "child-start", ReportOutcome::Started);
    let queued = f.submit("parent", "queued-before-backup", 0);
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        50,
    )
    .unwrap();
    f.work
        .db
        .execute(
            "INSERT INTO shared_handovers VALUES('legacy-pending',?1,?2,?3,?2,'pending',51,NULL)",
            rusqlite::params![parent.id, f.alice.user_id, f.bob.user_id],
        )
        .unwrap();
    f.work.fence_restored_backup(100).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Cancelled,
        "an old queued job may have run after the backup and must never regain admission"
    );
    assert_eq!(
        f.work.job(&f.alice, &parent.id).unwrap().state,
        JobState::Cancelling
    );
    assert_eq!(
        f.work.job(&f.alice, &child.job.id).unwrap().state,
        JobState::Cancelling
    );
    assert_eq!(
        f.work.job(&f.alice, &completed.id).unwrap().state,
        JobState::Succeeded
    );
    let status = f.work.attempt("WinE", &child.attempt_id).unwrap();
    assert_eq!(status.state, "stop_requested");
    assert!(status.uncertainty_reason.is_some());
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM shared_handovers WHERE state='pending'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    // Operator re-enabling environments does not revive the old queue.
    f.work
        .db
        .execute("UPDATE shared_environments SET enabled=1", [])
        .unwrap();
    assert!(f.work.claim("WinC", 101).unwrap().is_none());
    let stop_delivery = f.work.claim("WinE", 101).unwrap().unwrap();
    assert_eq!(stop_delivery.attempt_id, child.attempt_id);
    assert!(stop_delivery.stop_requested);
    assert!(
        f.work
            .report(
                "WinE",
                &Report {
                    event_id: "old-start-after-restore".into(),
                    attempt_id: child.attempt_id.clone(),
                    generation: child.generation,
                    outcome: ReportOutcome::Started
                },
                101
            )
            .is_err()
    );
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "stopped-after-restore".into(),
                attempt_id: child.attempt_id,
                generation: child.generation,
                outcome: finished(),
            },
            102,
        )
        .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &parent.id).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        f.work.job(&f.alice, &child.job.id).unwrap().state,
        JobState::Cancelled
    );
    assert!(f.work.claim("WinE", 103).unwrap().is_none());
}
