use super::*;
use rusqlite::params;

fn submit(f: &mut Fixture, device: &str, key: &str) -> Job {
    f.work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: key.into(),
                project_id: "alpha".into(),
                project_participation: Some(1),
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "parent".into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: key.into(),
                input: json!({"prompt":"Build the application"}),
                descendant_budget: 1,
                start_before_ms: None,
            },
            10,
            Some(device),
        )
        .unwrap()
}

fn legacy(f: &Fixture, root: &Job, state: &str) {
    f.work
        .db
        .execute(
            "INSERT INTO shared_handovers VALUES(?1,?2,?3,?4,?3,?5,11,NULL)",
            params![
                format!("legacy-{}", root.id),
                root.id,
                f.alice.user_id,
                f.bob.user_id,
                state
            ],
        )
        .unwrap();
}

#[test]
fn origin_operations_distinguish_same_user_devices_manager_and_shared_metadata() {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.admin.user_id,
        Some(ProjectRole::Manager),
        1,
    )
    .unwrap();
    let a = submit(&mut f, "WinA", "a-root");
    let d = submit(&mut f, "WinD", "d-root");
    for (job, owner, other) in [(&a, "WinA", "WinD"), (&d, "WinD", "WinA")] {
        assert!(f.work.require_job_origin(&f.alice, &job.id, owner).is_ok());
        assert!(f.work.require_job_origin(&f.alice, &job.id, other).is_err());
        assert!(f.work.require_job_origin(&f.admin, &job.id, owner).is_err());
        assert!(
            f.work
                .require_job_origin(&f.alice, &job.id, "WinC")
                .is_err()
        );
        let mut view = f.work.job_detail_at(&f.alice, &job.id, 12).unwrap();
        f.work
            .restrict_detail_to_origin(&f.alice, other, &mut view)
            .unwrap();
        assert_eq!(view["can_cancel"], false);
    }
    // Metadata retains its existing Contributor permission.
    f.work
        .rename_project_conversation(&f.alice, "alpha", &a.id, "Shared title", 1, 12)
        .unwrap();
    let parent = f.work.claim("WinC", 13).unwrap().unwrap();
    let (owner, other) = if parent.job.id == a.id {
        ("WinA", "WinD")
    } else {
        ("WinD", "WinA")
    };
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "child", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    assert!(
        f.work
            .require_job_origin(&f.alice, &child.job.id, owner)
            .is_ok()
    );
    assert!(
        f.work
            .require_job_origin(&f.alice, &child.job.id, other)
            .is_err()
    );
}

#[test]
fn continuation_and_latest_input_revision_require_the_original_device() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let mut f = Fixture::new();
    let root = submit(&mut f, "WinA", "continue-root");
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.admin.user_id,
        Some(ProjectRole::Manager),
        1,
    )
    .unwrap();
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    let bytes=serde_json::to_vec(&json!({"version":1,"job_id":root.id,"project_id":"alpha","environment_id":"parent","transcript":[]})).unwrap();
    f.work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &RunnerAssetUpload {
                generation: attempt.generation,
                kind: "archive".into(),
                base_sha256: None,
                upload: AssetUpload {
                    request_id: "archive".into(),
                    name: "history.json".into(),
                    content_base64: STANDARD.encode(&bytes),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                },
            },
            21,
        )
        .unwrap();
    f.report(&attempt, "done", finished());
    let terminal = f.work.job(&f.alice, &root.id).unwrap();
    let request = ContinueJob {
        request_id: "next".into(),
        expected_revision: terminal.revision,
        project_participation: Some(1),
        conversation_epoch: None,
        prompt: "Continue".into(),
        input_refs: vec![],
        start_before_ms: None,
    };
    for (actor, device) in [(&f.alice, "WinD"), (&f.admin, "WinA"), (&f.alice, "WinC")] {
        assert!(
            f.work
                .continue_job_for_device(actor, Some(device), &root.id, &request, 22)
                .is_err()
        );
    }
    let next = f
        .work
        .continue_job_for_device(&f.alice, Some("WinA"), &root.id, &request, 22)
        .unwrap();
    assert_eq!(
        f.work
            .continue_job_for_device(&f.alice, Some("WinA"), &root.id, &request, 23)
            .unwrap()
            .id,
        next.id
    );
    f.work.cancel(&f.alice, &next.id, 24).unwrap();
    let target = f.work.job(&f.alice, &next.id).unwrap();
    let revised = SubmitJob {
        request_id: "revised".into(),
        project_id: "alpha".into(),
        project_participation: Some(1),
        conversation_id: Some(root.id.clone()),
        revises_job_id: Some(target.id.clone()),
        expected_revised_revision: Some(target.revision),
        environment_id: "parent".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "Revised".into(),
        input: json!({"prompt":"Revised message"}),
        descendant_budget: 1,
        start_before_ms: None,
    };
    assert!(
        f.work
            .submit_for_device(&f.alice, &revised, 25, Some("WinD"))
            .is_err()
    );
    assert!(
        f.work
            .submit_for_device(&f.admin, &revised, 25, Some("WinA"))
            .is_err()
    );
    let accepted = f
        .work
        .submit_for_device(&f.alice, &revised, 25, Some("WinA"))
        .unwrap();
    assert_eq!(accepted.revises_job_id, Some(target.id));
}

#[test]
fn pending_transfer_migration_keeps_work_and_retired_requests_do_not_hold_queue() {
    let mut f = Fixture::new();
    let root = submit(&mut f, "WinA", "pending-root");
    legacy(&f, &root, "pending");
    f.work.db.pragma_update(None, "user_version", 24).unwrap();
    f.reopen();
    let state: String = f
        .work
        .db
        .query_row(
            "SELECT state FROM shared_handovers WHERE root_id=?1",
            [&root.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "cancelled");
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().assignee_id,
        f.alice.user_id
    );
    assert!(
        f.work
            .require_job_origin(&f.alice, &root.id, "WinA")
            .is_ok()
    );
    assert_eq!(f.work.claim("WinC", 12).unwrap().unwrap().job.id, root.id);
}

#[test]
fn completed_transfer_upgrade_requires_real_quiescence_and_preserves_history() {
    let mut f = Fixture::new();
    let root = submit(&mut f, "WinA", "settled-root");
    legacy(&f, &root, "completed");
    f.work.db.pragma_update(None, "user_version", 24).unwrap();
    let error = SharedWork::open(&f.dir.path().join("shared.sqlite3"))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains(&root.id));
    assert_eq!(
        f.work
            .db
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        24
    );
    f.work.cancel(&f.alice, &root.id, 12).unwrap();
    f.reopen();
    assert!(
        f.work
            .require_job_origin(&f.alice, &root.id, "WinA")
            .is_err()
    );
    assert!(f.work.require_job_origin(&f.bob, &root.id, "WinD").is_err());
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM shared_handovers WHERE root_id=?1 AND state='completed'",
                [&root.id],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().title,
        "settled-root"
    );
}

#[test]
fn unknown_legacy_origin_cannot_be_inferred_from_assignee_or_pc_name() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "unknown-origin", 0);
    legacy(&f, &root, "pending");
    f.work.db.pragma_update(None, "user_version", 24).unwrap();
    f.reopen();
    for device in ["WinA", "WinC", f.alice.display_name.as_str()] {
        assert!(
            f.work
                .require_job_origin(&f.alice, &root.id, device)
                .is_err()
        );
    }
    assert!(f.work.job(&f.alice, &root.id).is_ok());
    f.work.cancel(&f.alice, &root.id, 12).unwrap();
}

#[test]
fn legacy_quiescence_checks_attempts_and_services_even_when_root_is_terminal() {
    for retained in [false, true] {
        let mut f = Fixture::new();
        let root = submit(&mut f, "WinA", "hidden-resource");
        let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
        f.report(&attempt, "start", ReportOutcome::Started);
        if retained {
            f.report(
                &attempt,
                "retain",
                ReportOutcome::RetainService {
                    service_id: "preview".into(),
                    expires_at_ms: None,
                },
            );
            f.report(&attempt, "done", finished());
        } else {
            // A terminal job alone cannot prove the issued process has exited.
            f.work
                .db
                .execute(
                    "UPDATE shared_jobs SET state='succeeded' WHERE id=?1",
                    [&root.id],
                )
                .unwrap();
        }
        legacy(&f, &root, "completed");
        f.work.db.pragma_update(None, "user_version", 24).unwrap();
        assert!(SharedWork::open(&f.dir.path().join("shared.sqlite3")).is_err());
        assert_eq!(
            store::resource_occupancy(&f.work.db, "parent-slot").unwrap(),
            1
        );
    }
}
