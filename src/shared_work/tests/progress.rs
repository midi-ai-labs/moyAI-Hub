use super::*;

fn progress(attempt: &Assignment, revision: u64, text: &str) -> Report {
    serde_json::from_value(json!({
        "event_id": format!("progress:{revision}"),
        "attempt_id": attempt.attempt_id,
        "generation": attempt.generation,
        "outcome": {"kind":"progress", "revision":revision, "truncated":false,
            "items":[{"position":revision,"kind":"assistant_message","payload":{"content":[{"kind":"text","text":text}]}}]}
    }))
    .expect("the existing report route must accept bounded public progress")
}

#[test]
fn active_public_progress_is_visible_before_completion_without_changing_job_state() {
    let mut f = Fixture::new();
    let job = f.submit("parent", "progress-live", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    let before = serde_json::to_value(f.work.job(&f.alice, &job.id).unwrap()).unwrap();
    let report = progress(&attempt, 3, "The first file has been created.");
    f.work.report("WinC", &report, 21).unwrap();
    let page = serde_json::to_value(f.work.transcript(&f.alice, &job.id, 0, 100).unwrap()).unwrap();
    assert_eq!(
        page["activity"]["items"][0]["payload"]["content"][0]["text"],
        "The first file has been created."
    );
    assert_eq!(page["activity"]["observed_at_ms"], 21);
    assert_eq!(page["activity"]["revision"], 3);
    assert_eq!(page["items"], json!([]));
    assert_eq!(
        serde_json::to_value(f.work.job(&f.alice, &job.id).unwrap()).unwrap(),
        before
    );
}

#[test]
fn assignment_explicitly_advertises_progress_reports_and_legacy_defaults_false() {
    let mut f = Fixture::new();
    f.submit("parent", "progress-capability", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    let mut wire = serde_json::to_value(&attempt).unwrap();
    assert_eq!(wire["supports_progress_reports"], true);
    wire.as_object_mut()
        .unwrap()
        .remove("supports_progress_reports");
    let legacy: Assignment = serde_json::from_value(wire).unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["supports_progress_reports"],
        false
    );
}

#[test]
fn public_progress_rejects_internal_payload_fields_and_non_text_parts() {
    let mut f = Fixture::new();
    f.submit("parent", "progress-private-payload", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    for (kind, payload) in [
        (
            "assistant_message",
            json!({"content":[{"kind":"text","text":"public"}],"diagnostics":{"prompt":"private"}}),
        ),
        (
            "assistant_message",
            json!({"content":[{"kind":"thinking","text":"private"}]}),
        ),
        (
            "assistant_message",
            json!({"content":[{"kind":"image","image":{"base64":"private"}}]}),
        ),
        (
            "tool_output",
            json!({"call_id":"call","status":"completed","title":"done","output_text":"public","success":true,"metadata":{"private":"secret"}}),
        ),
        ("error", json!({"message":"public","internal":"private"})),
        (
            "file_change",
            json!({"summary":"public","snapshot":"private"}),
        ),
    ] {
        let report = Report {
            event_id: "progress:1".into(),
            attempt_id: attempt.attempt_id.clone(),
            generation: attempt.generation,
            outcome: ReportOutcome::Progress {
                revision: 1,
                items: vec![TranscriptItem {
                    position: 1,
                    kind: kind.into(),
                    payload,
                }],
                truncated: false,
            },
        };
        assert!(
            matches!(f.work.report("WinC", &report, 21), Err(WorkError::Invalid)),
            "{kind}"
        );
    }
    let mut public = vec![
        (
            "assistant_message",
            json!({"content":[{"kind":"text","text":"public"}]}),
        ),
        (
            "tool_call",
            json!({"call_id":"call","tool_name":"shell","arguments_json":"{\"command\":\"echo ok\"}"}),
        ),
        ("error", json!({"message":"public"})),
        ("file_change", json!({"summary":"public"})),
    ];
    for status in [
        "pending",
        "running",
        "completed",
        "declined",
        "cancelled",
        "failed",
    ] {
        public.push(("tool_output",json!({"call_id":"call","status":status,"title":"operation","output_text":"public","success":null})));
    }
    for (index, (kind, payload)) in public.into_iter().enumerate() {
        let revision = index as u64 + 1;
        let report = Report {
            event_id: format!("progress:{revision}"),
            attempt_id: attempt.attempt_id.clone(),
            generation: attempt.generation,
            outcome: ReportOutcome::Progress {
                revision,
                items: vec![TranscriptItem {
                    position: revision,
                    kind: kind.into(),
                    payload,
                }],
                truncated: false,
            },
        };
        f.work.report("WinC", &report, 22 + revision).unwrap();
    }
}

#[test]
fn progress_replay_is_stable_newer_replaces_and_reopen_keeps_only_latest_tail() {
    let mut f = Fixture::new();
    let job = f.submit("parent", "progress-replay", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    let first = progress(&attempt, 1, "one");
    f.work.report("WinC", &first, 21).unwrap();
    f.work.report("WinC", &first, 50).unwrap();
    assert_eq!(
        f.work
            .transcript(&f.alice, &job.id, 0, 10)
            .unwrap()
            .activity
            .unwrap()
            .observed_at_ms,
        21
    );
    assert!(matches!(
        f.work.report("WinC", &progress(&attempt, 1, "changed"), 51),
        Err(WorkError::Conflict)
    ));
    for revision in 2..=50 {
        f.work
            .report(
                "WinC",
                &progress(&attempt, revision, "newer"),
                50 + revision,
            )
            .unwrap();
    }
    assert!(matches!(
        f.work.report("WinC", &first, 101),
        Err(WorkError::Conflict)
    ));
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_attempt_progress", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_reports", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1,
        "display updates do not accumulate lifecycle receipts"
    );
    f.reopen();
    let page = f.work.transcript(&f.alice, &job.id, 1000, 1).unwrap();
    assert_eq!(
        page.activity.unwrap().revision,
        50,
        "archive pagination does not own the active tail cursor"
    );
    f.report(&attempt, "finished", finished());
    assert!(
        f.work
            .transcript(&f.alice, &job.id, 0, 10)
            .unwrap()
            .activity
            .is_none()
    );
    assert!(matches!(
        f.work.report("WinC", &progress(&attempt, 51, "late"), 102),
        Err(WorkError::Conflict)
    ));
}

#[test]
fn progress_rejects_wrong_owner_generation_stop_uncertainty_and_revoked_grants() {
    for change in [
        "owner",
        "generation",
        "assigned",
        "stop",
        "uncertain",
        "role",
        "environment",
        "runner",
        "authority",
    ] {
        let mut f = Fixture::new();
        let job = f.submit("parent", change, 0);
        let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
        if change != "assigned" {
            f.report(&attempt, "started", ReportOutcome::Started);
        }
        let mut report = progress(&attempt, 1, "public");
        if change != "assigned" {
            f.work.report("WinC", &report, 21).unwrap();
        }
        match change {
            "generation" => report.generation += 1,
            "stop" => {
                f.work.cancel(&f.alice, &job.id, 22).unwrap();
            }
            "uncertain" => {
                f.report(
                    &attempt,
                    "uncertain",
                    ReportOutcome::Uncertain {
                        reason: "offline".into(),
                    },
                );
            }
            "role" => {
                identity::set_project_role(
                    &f.work.db,
                    &f.admin,
                    "alpha",
                    &f.alice.user_id,
                    None,
                    22,
                )
                .unwrap();
            }
            "environment" => {
                f.work
                    .db
                    .execute(
                        "UPDATE shared_environments SET enabled=0 WHERE id='parent'",
                        [],
                    )
                    .unwrap();
            }
            "runner" => {
                f.work
                    .db
                    .execute(
                        "UPDATE shared_environments SET runner_id='WinOther' WHERE id='parent'",
                        [],
                    )
                    .unwrap();
            }
            "authority" => {
                f.work.db.execute("UPDATE shared_jobs SET authority_generation=authority_generation+1 WHERE id=?1",[&job.id]).unwrap();
            }
            _ => {}
        }
        let before = serde_json::to_value(store::load(&f.work.db, &job.id).unwrap()).unwrap();
        assert!(
            f.work
                .report(
                    if change == "owner" {
                        "WinOther"
                    } else {
                        "WinC"
                    },
                    &report,
                    23
                )
                .is_err(),
            "{change}"
        );
        assert_eq!(
            serde_json::to_value(store::load(&f.work.db, &job.id).unwrap()).unwrap(),
            before,
            "{change}"
        );
        if !matches!(change, "owner" | "generation") {
            let page = f.work.transcript(&f.admin, &job.id, 0, 10);
            assert!(
                page.is_err() || page.unwrap().activity.is_none(),
                "{change}"
            );
        }
    }
}

#[test]
fn progress_bounds_public_kinds_order_and_serialized_size() {
    let mut f = Fixture::new();
    let job = f.submit("parent", "progress-bounds", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    let valid = serde_json::to_value(progress(&attempt, 3, "safe")).unwrap();
    let mut invalid = Vec::new();
    for kind in [
        "request_diagnostics",
        "world_state",
        "compaction",
        "user_turn",
        "unknown",
    ] {
        let mut raw = valid.clone();
        raw["outcome"]["items"][0]["kind"] = json!(kind);
        invalid.push(raw);
    }
    let mut raw = valid.clone();
    raw["outcome"]["revision"] = json!(0);
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["items"][0]["position"] = json!(0);
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["revision"] = json!(4);
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["items"] = json!([]);
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["items"] = json!([
        raw["outcome"]["items"][0].clone(),
        raw["outcome"]["items"][0].clone()
    ]);
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["items"][0]["payload"] = json!("untyped");
    invalid.push(raw);
    let mut raw = valid.clone();
    raw["outcome"]["items"][0]["payload"]["content"][0]["text"] = json!("x".repeat(16 * 1024));
    invalid.push(raw);
    for (count, bytes) in [(13, 1), (12, 6 * 1024)] {
        let mut raw = valid.clone();
        raw["outcome"]["revision"] = json!(count);
        raw["outcome"]["items"] = json!(
            (1..=count)
                .map(
                    |i| json!({"position":i,"kind":"error","payload":{"message":"x".repeat(bytes)}})
                )
                .collect::<Vec<_>>()
        );
        invalid.push(raw);
    }
    for raw in invalid {
        let report: Report = serde_json::from_value(raw).unwrap();
        assert!(matches!(
            f.work.report("WinC", &report, 21),
            Err(WorkError::Invalid)
        ));
    }
    let mut extra = valid.clone();
    extra["outcome"]["items"][0]["internal"] = json!("private");
    assert!(serde_json::from_value::<Report>(extra).is_err());
    f.work
        .report("WinC", &serde_json::from_value(valid).unwrap(), 22)
        .unwrap();
    assert!(f.work.transcript(&f.bob, &job.id, 0, 10).is_err());
}

#[test]
fn root_transcript_shows_only_current_awaited_child_and_never_old_attempt_after_resume() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "progress-tree", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&parent, "parent-started", ReportOutcome::Started);
    f.work
        .report("WinC", &progress(&parent, 1, "parent"), 21)
        .unwrap();
    f.report(
        &parent,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "Child".into(),
            child_input: json!({}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinE", 22).unwrap().unwrap();
    f.report(&child, "child-started", ReportOutcome::Started);
    f.work
        .report("WinE", &progress(&child, 1, "child"), 23)
        .unwrap();
    let activity = f
        .work
        .transcript(&f.alice, &root.id, 0, 10)
        .unwrap()
        .activity
        .unwrap();
    assert_eq!(activity.job_id, child.job.id);
    assert_eq!(activity.environment_id, "solver");
    assert!(matches!(
        f.work.report("WinC", &progress(&parent, 2, "stale"), 24),
        Err(WorkError::Conflict)
    ));
    f.report(&child, "child-finished", finished());
    let resumed = f.work.claim("WinC", 25).unwrap().unwrap();
    assert_ne!(resumed.attempt_id, parent.attempt_id);
    f.report(&resumed, "resumed-started", ReportOutcome::Started);
    assert!(
        f.work
            .transcript(&f.alice, &root.id, 0, 10)
            .unwrap()
            .activity
            .is_none()
    );
    f.work
        .report("WinC", &progress(&resumed, 3, "resumed"), 26)
        .unwrap();
    assert_eq!(
        f.work
            .transcript(&f.alice, &root.id, 0, 10)
            .unwrap()
            .activity
            .unwrap()
            .attempt_id,
        resumed.attempt_id
    );
    assert!(
        f.work
            .transcript(&f.alice, &child.job.id, 0, 10)
            .unwrap()
            .activity
            .is_none()
    );
}

#[test]
fn settled_archive_takes_precedence_and_v22_migration_preserves_existing_receipts() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let mut f = Fixture::new();
    let job = f.submit("parent", "progress-archive", 0);
    let attempt = f.work.claim("WinC", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    f.work
        .db
        .execute_batch("ALTER TABLE shared_projects DROP COLUMN overview; ALTER TABLE shared_jobs DROP COLUMN project_context_json; DROP TABLE shared_attempt_progress; PRAGMA user_version=22;")
        .unwrap();
    f.reopen();
    assert!(
        f.work
            .transcript(&f.alice, &job.id, 0, 10)
            .unwrap()
            .activity
            .is_none()
    );
    assert_eq!(
        f.work
            .report(
                "WinC",
                &Report {
                    event_id: "started".into(),
                    attempt_id: attempt.attempt_id.clone(),
                    generation: attempt.generation,
                    outcome: ReportOutcome::Started
                },
                21
            )
            .unwrap()
            .state,
        JobState::Running
    );
    f.work
        .report("WinC", &progress(&attempt, 1, "in progress"), 22)
        .unwrap();
    let content = serde_json::to_vec(&json!({"version":1,"job_id":job.id,"project_id":job.project_id,"environment_id":job.environment_id,"transcript":[{"kind":"assistant_message","payload":{"text":"final"}}]})).unwrap();
    f.work
        .runner_upload_asset(
            "WinC",
            &attempt.attempt_id,
            &RunnerAssetUpload {
                generation: attempt.generation,
                kind: "archive".into(),
                base_sha256: None,
                upload: AssetUpload {
                    request_id: "final-archive".into(),
                    name: "canonical.json".into(),
                    content_base64: STANDARD.encode(&content),
                    sha256: format!("{:x}", Sha256::digest(&content)),
                },
            },
            23,
        )
        .unwrap();
    let page = f.work.transcript(&f.alice, &job.id, 0, 10).unwrap();
    assert!(page.activity.is_none());
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].payload["text"], "final");
    f.report(&attempt, "finished", finished());
    assert!(
        f.work
            .transcript(&f.alice, &job.id, 0, 10)
            .unwrap()
            .activity
            .is_none()
    );
}
