use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

fn local_request(request_id: &str, project: &str, origin: &str) -> SubmitJob {
    SubmitJob {
        request_id: request_id.into(),
        project_id: project.into(),
        project_participation: None,
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: "solver".into(),
        origin_session_ref: Some(origin.into()),
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: request_id.into(),
        input: json!({"prompt":"test"}),
        descendant_budget: 1,
        start_before_ms: None,
    }
}

#[test]
fn origin_projection_requires_exact_device_requestor_and_current_project_grant() {
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
    let alpha = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("alpha-local", "alpha", "session"),
            10,
            Some("WinA"),
        )
        .unwrap();
    let assigned = f.work.claim("WinE", 11).unwrap().unwrap();
    assert_eq!(assigned.job.id, alpha.id);
    f.report(&assigned, "start", ReportOutcome::Started);
    for index in 0..21 {
        let contents = format!("result-{index}");
        f.work
            .runner_upload_asset(
                "WinE",
                &assigned.attempt_id,
                &RunnerAssetUpload {
                    generation: assigned.generation,
                    kind: "artifact".into(),
                    base_sha256: None,
                    upload: AssetUpload {
                        request_id: format!("origin-artifact-{index}"),
                        name: format!("result-{index}.txt"),
                        content_base64: STANDARD.encode(contents.as_bytes()),
                        sha256: format!("{:x}", Sha256::digest(contents.as_bytes())),
                    },
                },
                19,
            )
            .unwrap();
    }
    f.report(
        &assigned,
        "preview",
        ReportOutcome::RetainService {
            service_id: "service-alpha".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&assigned, "done", finished());
    let beta = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("beta-local", "beta", "session"),
            12,
            Some("WinA"),
        )
        .unwrap();
    let other_pc = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("other-pc", "alpha", "session"),
            13,
            Some("WinB"),
        )
        .unwrap();
    let bob = f
        .work
        .submit_for_device(
            &f.bob,
            &local_request("other-person", "beta", "session"),
            14,
            Some("WinA"),
        )
        .unwrap();
    let work = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "session", None, None, 21)
        .unwrap();
    assert!(!work.hidden_active_work);
    assert_eq!(work.jobs.len(), 2);
    assert!(
        work.jobs
            .iter()
            .any(|item| item.project_id == "alpha" && item.job.id == alpha.id)
    );
    assert!(
        work.jobs
            .iter()
            .any(|item| item.project_id == "beta" && item.job.id == beta.id)
    );
    assert!(
        work.jobs
            .iter()
            .all(|item| item.job.id != other_pc.id && item.job.id != bob.id)
    );
    let alpha_card = work
        .jobs
        .iter()
        .find(|item| item.job.id == alpha.id)
        .unwrap();
    assert_eq!(alpha_card.artifacts.len(), 20);
    assert!(alpha_card.more_artifacts);
    assert_eq!(alpha_card.artifacts[0].kind, "artifact");
    assert_eq!(alpha_card.artifacts[0].project_id, "alpha");
    assert!(
        work.jobs
            .iter()
            .find(|item| item.job.id == beta.id)
            .unwrap()
            .artifacts
            .is_empty()
    );
    assert_eq!(work.retained_services.len(), 1);
    assert_eq!(work.retained_services[0].project_id, "alpha");
    assert_eq!(
        work.retained_services[0].service.service_id,
        "service-alpha"
    );
    assert_eq!(
        work.retained_services[0].service.conversation_id,
        alpha.conversation_id
    );
    assert!(work.retained_services[0].service.can_stop);
    let other_origin = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "different", None, None, 21)
        .unwrap();
    assert!(other_origin.jobs.is_empty());
    assert!(!other_origin.hidden_active_work);
    assert_eq!(
        f.work
            .origin_work_for_device(&f.alice, "WinB", "session", None, None, 21)
            .unwrap()
            .jobs[0]
            .job
            .id,
        other_pc.id
    );
    assert_eq!(
        f.work
            .origin_work_for_device(&f.bob, "WinA", "session", None, None, 21)
            .unwrap()
            .jobs[0]
            .job
            .id,
        bob.id
    );
    assert!(
        f.work
            .origin_work_for_device(&f.alice, "WinA", "session", None, Some("service-alpha"), 21)
            .unwrap()
            .retained_services
            .is_empty()
    );

    identity::set_project_role(&f.work.db, &f.admin, "beta", &f.alice.user_id, None, 22).unwrap();
    let after_grant_loss = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "session", None, None, 22)
        .unwrap();
    assert_eq!(after_grant_loss.jobs.len(), 1);
    assert_eq!(after_grant_loss.jobs[0].project_id, "alpha");
    assert!(after_grant_loss.hidden_active_work);
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.alice.user_id,
        Some(ProjectRole::Reader),
        23,
    )
    .unwrap();
    let reader = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "session", None, None, 23)
        .unwrap();
    assert_eq!(reader.retained_services.len(), 1);
    assert!(!reader.retained_services[0].service.can_stop);
    f.work.db.execute_batch("UPDATE shared_projects SET legacy_devices_unrestricted=0 WHERE id='alpha'; INSERT INTO shared_project_devices(project_id,device_id,can_control,can_execute) VALUES('alpha','WinA',1,0);").unwrap();
    assert_eq!(
        f.work
            .origin_work_for_device(&f.alice, "WinA", "session", None, None, 24)
            .unwrap()
            .jobs
            .len(),
        1
    );
    f.work.db.execute("UPDATE shared_project_devices SET can_control=0 WHERE project_id='alpha' AND device_id='WinA'", []).unwrap();
    let revoked = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "session", None, None, 25)
        .unwrap();
    assert!(revoked.jobs.is_empty());
    assert!(revoked.retained_services.is_empty());
    assert!(revoked.hidden_active_work);
    assert!(
        !f.work
            .origin_work_for_device(&f.bob, "WinB", "session", None, None, 25)
            .unwrap()
            .hidden_active_work
    );
}

#[test]
fn hidden_origin_presence_requires_live_work_and_the_original_device_and_owner() {
    let mut f = Fixture::new();
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("terminal", "alpha", "private"),
            10,
            Some("WinA"),
        )
        .unwrap();
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(
        &attempt,
        "retain",
        ReportOutcome::RetainService {
            service_id: "hidden-preview".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&attempt, "done", finished());
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 20).unwrap();
    let hidden = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "private", None, None, 21)
        .unwrap();
    assert!(hidden.jobs.is_empty());
    assert!(hidden.retained_services.is_empty());
    assert!(hidden.hidden_active_work);
    assert!(
        !f.work
            .origin_work_for_device(&f.alice, "WinB", "private", None, None, 21)
            .unwrap()
            .hidden_active_work
    );
    assert!(
        !f.work
            .origin_work_for_device(&f.bob, "WinA", "private", None, None, 21)
            .unwrap()
            .hidden_active_work
    );
    f.report(
        &attempt,
        "stopped",
        ReportOutcome::ServiceStopped {
            service_id: "hidden-preview".into(),
        },
    );
    let terminal = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "private", None, None, 22)
        .unwrap();
    assert!(!terminal.hidden_active_work);
    assert_eq!(root.origin_session_ref.as_deref(), Some("private"));
}

#[test]
fn origin_projection_pages_jobs_and_v18_rows_remain_unattributed_after_migration() {
    let mut f = Fixture::new();
    for index in 0..52 {
        f.work
            .submit_for_device(
                &f.alice,
                &local_request(&format!("job-{index:03}"), "alpha", "many"),
                10,
                Some("WinA"),
            )
            .unwrap();
    }
    let first = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "many", None, None, 20)
        .unwrap();
    assert_eq!(first.jobs.len(), 50);
    assert!(first.next_job_before.is_some());
    assert!(first.next_service_before.is_none());
    let second = f
        .work
        .origin_work_for_device(
            &f.alice,
            "WinA",
            "many",
            first.next_job_before.as_deref(),
            None,
            20,
        )
        .unwrap();
    assert_eq!(second.jobs.len(), 2);
    assert!(second.next_job_before.is_none());
    assert!(
        second
            .jobs
            .iter()
            .all(|item| first.jobs.iter().all(|old| old.job.id != item.job.id))
    );

    let legacy = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("legacy", "alpha", "old-session"),
            21,
            Some("WinA"),
        )
        .unwrap();
    f.work.db.execute_batch("ALTER TABLE shared_projects DROP COLUMN overview; ALTER TABLE shared_jobs DROP COLUMN project_context_json; DROP TABLE shared_attempt_progress; DROP TABLE shared_conversation_details; DROP INDEX shared_jobs_controller_participation; ALTER TABLE shared_jobs DROP COLUMN controller_device_id; ALTER TABLE shared_jobs DROP COLUMN controller_participation_generation; ALTER TABLE shared_jobs DROP COLUMN revises_job_id; ALTER TABLE shared_projects DROP COLUMN deleted_at_ms; ALTER TABLE shared_project_devices DROP COLUMN participation_generation; DROP INDEX shared_jobs_origin_turn; DROP INDEX shared_jobs_conversation_history; DROP TABLE shared_project_stop_receipts; DROP TABLE shared_conversation_stop_fences; DROP TABLE shared_origin_turns; DROP TABLE shared_origin_scopes; ALTER TABLE shared_jobs DROP COLUMN origin_turn_revision; ALTER TABLE shared_jobs DROP COLUMN origin_turn_epoch; ALTER TABLE shared_jobs DROP COLUMN origin_turn_ref; DROP INDEX shared_jobs_origin_device; ALTER TABLE shared_jobs DROP COLUMN required_capability; ALTER TABLE shared_jobs DROP COLUMN origin_device_id; PRAGMA user_version=18;").unwrap();
    f.reopen();
    let migrated_version: u32 = f
        .work
        .db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(migrated_version, 26);
    let old_origin_device: Option<String> = f
        .work
        .db
        .query_row(
            "SELECT origin_device_id FROM shared_jobs WHERE id=?1",
            [&legacy.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(old_origin_device.is_none());
    let migrated_requirement: String = f
        .work
        .db
        .query_row(
            "SELECT required_capability FROM shared_jobs WHERE id=?1",
            [&legacy.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migrated_requirement, store::MULTI_DEVICE_SESSION_CAPABILITY);
    assert!(
        f.work
            .origin_work_for_device(&f.alice, "WinA", "old-session", None, None, 22)
            .unwrap()
            .jobs
            .is_empty()
    );
    assert!(matches!(
        f.work.submit_for_device(
            &f.alice,
            &local_request("legacy", "alpha", "old-session"),
            22,
            Some("WinA")
        ),
        Err(WorkError::Forbidden)
    ));
    let new = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("post-migration", "alpha", "old-session"),
            23,
            Some("WinA"),
        )
        .unwrap();
    let projected = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "old-session", None, None, 24)
        .unwrap();
    assert_eq!(projected.jobs.len(), 1);
    assert_eq!(projected.jobs[0].job.id, new.id);
}

#[test]
fn origin_device_binding_survives_child_work() {
    let mut f = Fixture::new();
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("root", "alpha", "session-child"),
            10,
            Some("WinA"),
        )
        .unwrap();
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(
        &attempt,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "parent".into(),
            child_title: "Child".into(),
            child_input: json!({"prompt":"check"}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinC", 21).unwrap().unwrap().job;
    let root_device: String = f
        .work
        .db
        .query_row(
            "SELECT origin_device_id FROM shared_jobs WHERE id=?1",
            [&root.id],
            |row| row.get(0),
        )
        .unwrap();
    let child_device: String = f
        .work
        .db
        .query_row(
            "SELECT origin_device_id FROM shared_jobs WHERE id=?1",
            [&child.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(root_device, "WinA");
    assert_eq!(child_device, root_device);
    assert_eq!(
        f.work
            .origin_work_for_device(&f.alice, "WinA", "session-child", None, None, 22)
            .unwrap()
            .jobs
            .len(),
        2
    );
}

#[test]
fn origin_projection_preserves_origin_continuation_beside_independent_users_work() {
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
    let root = f
        .work
        .submit_for_device(
            &f.alice,
            &local_request("alice-root", "alpha", "shared-reference"),
            10,
            Some("WinA"),
        )
        .unwrap();
    let first = f.work.claim("WinE", 12).unwrap().unwrap();
    assert_eq!(first.job.assignee_id, f.alice.user_id);
    f.report(&first, "start", ReportOutcome::Started);
    let archive = json!({
        "version":1,
        "job_id":root.id,
        "project_id":"alpha",
        "environment_id":"solver",
        "transcript":[{"kind":"message","payload":{"text":"original result"}}]
    });
    let archive_bytes = serde_json::to_vec(&archive).unwrap();
    f.work
        .runner_upload_asset(
            "WinE",
            &first.attempt_id,
            &RunnerAssetUpload {
                generation: first.generation,
                kind: "archive".into(),
                base_sha256: None,
                upload: AssetUpload {
                    request_id: "root-archive".into(),
                    name: "history.json".into(),
                    content_base64: STANDARD.encode(&archive_bytes),
                    sha256: format!("{:x}", Sha256::digest(&archive_bytes)),
                },
            },
            20,
        )
        .unwrap();
    f.report(&first, "done", finished());
    let terminal = f.work.job(&f.bob, &root.id).unwrap();
    let continued = f
        .work
        .continue_job_for_device(
            &f.alice,
            Some("WinA"),
            &root.id,
            &ContinueJob {
                request_id: "bob-continues".into(),
                expected_revision: terminal.revision,
                project_participation: None,
                conversation_epoch: None,
                prompt: "Continue the original conversation".into(),
                input_refs: Vec::new(),
                start_before_ms: None,
            },
            21,
        )
        .unwrap();
    assert_eq!(continued.conversation_id, root.id);
    assert_eq!(continued.requestor_id, f.alice.user_id);
    let second = f.work.claim("WinE", 22).unwrap().unwrap();
    assert_eq!(second.job.id, continued.id);
    f.report(&second, "continue-start", ReportOutcome::Started);
    let artifact = b"result from continued work";
    f.work
        .runner_upload_asset(
            "WinE",
            &second.attempt_id,
            &RunnerAssetUpload {
                generation: second.generation,
                kind: "artifact".into(),
                base_sha256: None,
                upload: AssetUpload {
                    request_id: "continued-artifact".into(),
                    name: "result.txt".into(),
                    content_base64: STANDARD.encode(artifact),
                    sha256: format!("{:x}", Sha256::digest(artifact)),
                },
            },
            23,
        )
        .unwrap();
    f.report(
        &second,
        "continue-preview",
        ReportOutcome::RetainService {
            service_id: "continued-preview".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&second, "continue-done", finished());
    let bob_root = f
        .work
        .submit_for_device(
            &f.bob,
            &local_request("bob-root", "alpha", "shared-reference"),
            24,
            Some("WinA"),
        )
        .unwrap();

    let alice = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "shared-reference", None, None, 25)
        .unwrap();
    assert_eq!(alice.jobs.len(), 2);
    assert!(alice.jobs.iter().any(|view| view.job.id == root.id));
    assert!(
        alice
            .jobs
            .iter()
            .any(|view| { view.job.id == continued.id && view.artifacts.len() == 1 })
    );
    assert!(alice.jobs.iter().all(|view| view.job.id != bob_root.id));
    assert_eq!(alice.retained_services.len(), 1);
    assert_eq!(
        alice.retained_services[0].service.service_id,
        "continued-preview"
    );
    assert!(alice.retained_services[0].service.can_stop);
    assert!(matches!(
        f.work
            .request_conversation_services_stop(&f.bob, "alpha", &root.id, "alice-stop", 25),
        Err(WorkError::Forbidden)
    ));
    let bob = f
        .work
        .origin_work_for_device(&f.bob, "WinA", "shared-reference", None, None, 25)
        .unwrap();
    assert_eq!(bob.jobs.len(), 1);
    assert_eq!(bob.jobs[0].job.id, bob_root.id);
    assert!(bob.retained_services.is_empty());
    assert!(
        f.work
            .origin_work_for_device(&f.alice, "WinB", "shared-reference", None, None, 25)
            .unwrap()
            .jobs
            .is_empty()
    );

    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET conversation_id='missing-origin' WHERE id=?1",
            [&continued.id],
        )
        .unwrap();
    let missing_anchor = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "shared-reference", None, None, 25)
        .unwrap();
    assert_eq!(missing_anchor.jobs.len(), 1);
    assert_eq!(missing_anchor.jobs[0].job.id, root.id);
    assert!(missing_anchor.retained_services.is_empty());

    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 26).unwrap();
    let revoked = f
        .work
        .origin_work_for_device(&f.alice, "WinA", "shared-reference", None, None, 27)
        .unwrap();
    assert!(revoked.jobs.is_empty() && revoked.retained_services.is_empty());
}
