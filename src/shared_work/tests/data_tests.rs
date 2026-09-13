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
                environment_id: "parent".into(),
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
                    environment_id: "solver".into(),
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
    let root = with_input(&mut f, "continue-data", &input.id);
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
    let request = ContinueJob {
        request_id: "next-root".into(),
        expected_revision: terminal.revision,
        prompt: "Continue from this conversation".into(),
        input_refs: vec![input.id.clone()],
        start_before_ms: None,
    };
    let next = f
        .work
        .continue_job(&f.alice, &root.id, &request, 41)
        .unwrap();
    assert_ne!(next.id, root.id);
    assert_eq!(next.continued_from.as_deref(), Some(root.id.as_str()));
    assert_eq!(next.quota_owner_id, root.quota_owner_id);
    assert_eq!(next.start_before_ms, 41 + DEFAULT_START_WINDOW_MS);
    assert!(next.start_before_ms > terminal.start_before_ms);
    assert_eq!(
        f.work
            .continue_job(&f.alice, &root.id, &request, 42)
            .unwrap()
            .id,
        next.id
    );
    assert_eq!(
        f.work
            .continue_job(&f.alice, &root.id, &request, next.start_before_ms + 1)
            .unwrap()
            .start_before_ms,
        next.start_before_ms
    );
    let invalid_deadline = ContinueJob {
        request_id: "expired-continuation".into(),
        start_before_ms: Some(42),
        ..request.clone()
    };
    assert!(matches!(
        f.work
            .continue_job(&f.alice, &root.id, &invalid_deadline, 42),
        Err(WorkError::StartDeadline)
    ));
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().revision,
        terminal.revision
    );
    assert_eq!(
        f.work.preview_retention(&f.admin, 50).unwrap()["asset_count"],
        0
    );
    prune(&mut f, 50, 60);
    let next_attempt = f.work.claim("WinC", 61).unwrap().unwrap();
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
                    environment_id: "parent".into(),
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
                10
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
    let before_handover = f.work.job(&f.alice, &parent.id).unwrap();
    f.work
        .handover(
            &f.alice,
            &parent.id,
            &HandoverRequest {
                expected_revision: before_handover.revision,
                new_assignee_id: f.bob.user_id.clone(),
            },
            51,
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
