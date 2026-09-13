use super::*;

fn request(key: &str, environment: &str, deadline: Option<u64>) -> SubmitJob {
    SubmitJob {
        request_id: key.into(),
        project_id: "alpha".into(),
        environment_id: environment.into(),
        title: key.into(),
        input: json!({"version":1,"prompt":"test"}),
        descendant_budget: 2,
        start_before_ms: deadline,
    }
}
fn report_at(
    f: &mut Fixture,
    assignment: &Assignment,
    event: &str,
    outcome: ReportOutcome,
    now: u64,
) -> Result<Job> {
    f.work.report(
        &assignment.runner_id,
        &Report {
            event_id: event.into(),
            attempt_id: assignment.attempt_id.clone(),
            generation: assignment.generation,
            outcome,
        },
        now,
    )
}

#[test]
fn shared_deadline_stale_queued_work_cannot_start_when_runner_returns() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "old-offline-request", 0);
    let after_default_window = job.created_at_ms + 24 * 60 * 60 * 1000;
    assert!(
        f.work
            .claim("WinE", after_default_window)
            .unwrap()
            .is_none(),
        "the returning Runner must not start a request after its default start deadline"
    );
    let expired = f.work.job(&f.alice, &job.id).unwrap();
    assert_eq!(expired.state, JobState::Failed);
    assert_eq!(
        expired.result.as_ref().unwrap()["error"],
        "start_deadline_exceeded"
    );
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    assert!(
        f.work.job_detail(&f.alice, &job.id).unwrap()["wait_reason"]
            .as_str()
            .unwrap()
            .contains("開始期限")
    );
}

#[test]
fn shared_deadline_is_finite_persisted_and_receipt_replay_never_extends_it() {
    let mut f = Fixture::new();
    let original = request("default", "solver", None);
    assert!(
        serde_json::to_value(&original)
            .unwrap()
            .get("start_before_ms")
            .is_none()
    );
    let job = f.work.submit(&f.alice, &original, 10).unwrap();
    assert_eq!(job.start_before_ms, 10 + DEFAULT_START_WINDOW_MS);
    f.reopen();
    assert_eq!(
        f.work
            .submit(&f.alice, &original, job.start_before_ms + 1)
            .unwrap()
            .start_before_ms,
        job.start_before_ms
    );
    let changed = SubmitJob {
        start_before_ms: Some(job.start_before_ms + 1000),
        ..original
    };
    assert!(matches!(
        f.work.submit(&f.alice, &changed, 20),
        Err(WorkError::Conflict)
    ));
    for value in [0, 30, u64::MAX] {
        assert!(
            f.work
                .submit(
                    &f.alice,
                    &request(&format!("invalid-{value}"), "solver", Some(value)),
                    30
                )
                .is_err()
        );
    }
    let explicit = f
        .work
        .submit(&f.alice, &request("explicit", "parent", Some(100)), 31)
        .unwrap();
    assert_eq!(explicit.start_before_ms, 100);
    assert_eq!(
        f.work.job_detail(&f.alice, &explicit.id).unwrap()["start_before_ms"],
        100
    );
}

#[test]
fn shared_deadline_fences_unstarted_but_holds_unknown_and_allows_started_work_to_finish() {
    let mut f = Fixture::new();
    let job = f
        .work
        .submit(&f.alice, &request("unstarted", "solver", Some(100)), 10)
        .unwrap();
    let assigned = f.work.claim("WinE", 20).unwrap().unwrap();
    assert!(report_at(&mut f, &assigned, "late-start", ReportOutcome::Started, 100).is_err());
    assert_eq!(
        f.work.job(&f.alice, &job.id).unwrap().state,
        JobState::Failed
    );
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    assert!(
        report_at(
            &mut f,
            &assigned,
            "late-start-again",
            ReportOutcome::Started,
            101
        )
        .is_err()
    );
    let unknown = f
        .work
        .submit(&f.alice, &request("unknown", "solver", Some(200)), 110)
        .unwrap();
    let assigned = f.work.claim("WinE", 120).unwrap().unwrap();
    report_at(
        &mut f,
        &assigned,
        "uncertain",
        ReportOutcome::Uncertain {
            reason: "delivery needs reconciliation".into(),
        },
        130,
    )
    .unwrap();
    assert!(report_at(&mut f, &assigned, "late-start", ReportOutcome::Started, 200).is_err());
    let held = f.work.assignments("WinE").unwrap();
    assert_eq!(held.len(), 1);
    assert!(held[0].stop_requested);
    assert!(!f.work.job(&f.alice, &unknown.id).unwrap().state.terminal());
    report_at(
        &mut f,
        &assigned,
        "stopped",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
        210,
    )
    .unwrap();
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    let started = f
        .work
        .submit(
            &f.alice,
            &request("already-started", "solver", Some(300)),
            220,
        )
        .unwrap();
    let assigned = f.work.claim("WinE", 230).unwrap().unwrap();
    report_at(&mut f, &assigned, "start", ReportOutcome::Started, 240).unwrap();
    f.work.claim("WinE", 301).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &started.id).unwrap().state,
        JobState::Running
    );
    // An acknowledged start stays acknowledged after the deadline; replay cannot allocate anew.
    report_at(&mut f, &assigned, "start", ReportOutcome::Started, 301).unwrap();
    report_at(&mut f, &assigned, "finish", finished(), 310).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &started.id).unwrap().state,
        JobState::Succeeded
    );
}

#[test]
fn shared_deadline_expired_child_settles_waiting_parent_and_cancellation_never_reopens_work() {
    let mut f = Fixture::new();
    let root = f
        .work
        .submit(&f.alice, &request("late-child", "parent", Some(100)), 10)
        .unwrap();
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
    let result = report_at(
        &mut f,
        &parent,
        "yield-after-deadline",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "expired child".into(),
            child_input: json!({"prompt":"child"}),
            resources_released: true,
        },
        100,
    )
    .unwrap();
    assert_eq!(result.state, JobState::Failed);
    assert_eq!(
        f.work
            .job(&f.alice, result.awaiting_child_id.as_ref().unwrap())
            .unwrap()
            .state,
        JobState::Failed
    );
    assert!(f.work.claim("WinE", 101).unwrap().is_none());
    assert_eq!(
        f.work.cancel(&f.alice, &root.id, 102).unwrap().state,
        JobState::Failed
    );

    let cancelled = f
        .work
        .submit(&f.alice, &request("cancel-first", "solver", Some(200)), 110)
        .unwrap();
    let attempt = f.work.claim("WinE", 120).unwrap().unwrap();
    assert_eq!(
        f.work.cancel(&f.alice, &cancelled.id, 130).unwrap().state,
        JobState::Cancelling
    );
    f.work.expire_start_deadlines(201).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &cancelled.id).unwrap().state,
        JobState::Cancelling
    );
    assert!(f.work.assignments("WinE").unwrap()[0].stop_requested);
    report_at(
        &mut f,
        &attempt,
        "stopped",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
        202,
    )
    .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &cancelled.id).unwrap().state,
        JobState::Cancelled
    );
}

#[test]
fn shared_deadline_offline_reconciliation_is_persisted_and_unknown_slots_still_block_capacity() {
    let mut f = Fixture::new();
    let unknown = f
        .work
        .submit(&f.alice, &request("unknown-held", "solver", Some(100)), 10)
        .unwrap();
    let attempt = f.work.claim("WinE", 20).unwrap().unwrap();
    report_at(
        &mut f,
        &attempt,
        "uncertain",
        ReportOutcome::Uncertain {
            reason: "unknown".into(),
        },
        30,
    )
    .unwrap();
    let queued = f
        .work
        .submit(
            &f.alice,
            &request("offline-queued", "parent", Some(100)),
            40,
        )
        .unwrap();
    f.work.expire_start_deadlines(100).unwrap();
    f.reopen();
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Failed
    );
    assert_eq!(
        f.work.job(&f.alice, &unknown.id).unwrap().state,
        JobState::Assigned
    );
    let detail = f.work.job_detail(&f.alice, &unknown.id).unwrap();
    assert!(
        detail["wait_reason"]
            .as_str()
            .unwrap()
            .contains("利用枠を保持")
    );
    let next = f
        .work
        .submit(&f.alice, &request("new-solver", "solver", Some(300)), 110)
        .unwrap();
    let held = f.work.claim("WinE", 120).unwrap().unwrap();
    assert_eq!(held.attempt_id, attempt.attempt_id);
    assert!(held.stop_requested);
    assert_eq!(
        f.work.job(&f.alice, &next.id).unwrap().state,
        JobState::Queued
    );
}

#[test]
fn shared_deadline_covers_parent_resume_without_stopping_an_already_started_child() {
    let mut f = Fixture::new();
    let root = f
        .work
        .submit(&f.alice, &request("parent", "parent", Some(100)), 10)
        .unwrap();
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
    let yielded = report_at(
        &mut f,
        &parent,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "child".into(),
            child_input: json!({"version":1,"prompt":"child"}),
            resources_released: true,
        },
        20,
    )
    .unwrap();
    let child = f.work.claim("WinE", 30).unwrap().unwrap();
    assert_eq!(child.job.start_before_ms, root.start_before_ms);
    report_at(&mut f, &child, "start", ReportOutcome::Started, 40).unwrap();
    assert!(f.work.claim("WinC", 100).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::WaitingChild
    );
    assert_eq!(
        f.work
            .job(&f.alice, yielded.awaiting_child_id.as_ref().unwrap())
            .unwrap()
            .state,
        JobState::Running
    );
    report_at(&mut f, &child, "finish", finished(), 110).unwrap();
    assert!(f.work.claim("WinC", 111).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Failed
    );
    assert_eq!(
        f.work.job(&f.alice, &child.job.id).unwrap().state,
        JobState::Succeeded
    );
}

#[test]
fn shared_deadline_v9_forward_migration_is_finite_and_keeps_started_authority() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "legacy-root", 1);
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
    report_at(
        &mut f,
        &parent,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "solver".into(),
            child_title: "child".into(),
            child_input: json!({"prompt":"child"}),
            resources_released: true,
        },
        20,
    )
    .unwrap();
    let child = f.work.claim("WinE", 30).unwrap().unwrap();
    report_at(&mut f, &child, "start", ReportOutcome::Started, 40).unwrap();
    f.work.db.execute_batch("DROP INDEX shared_jobs_start_deadline; ALTER TABLE shared_jobs DROP COLUMN start_before_ms; PRAGMA user_version=9;").unwrap();
    f.reopen();
    assert_eq!(
        f.work
            .db
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        10
    );
    for id in [&root.id, &child.job.id] {
        assert_eq!(
            f.work.job(&f.alice, id).unwrap().start_before_ms,
            10 + DEFAULT_START_WINDOW_MS
        );
    }
    assert_eq!(
        f.work.attempt("WinE", &child.attempt_id).unwrap().state,
        "running"
    );
    f.reopen();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().start_before_ms,
        10 + DEFAULT_START_WINDOW_MS
    );
}

#[test]
fn shared_deadline_handover_keeps_uncertain_child_capacity_until_exact_stop() {
    for (expired, manual) in [(false, false), (false, true), (true, false)] {
        let mut f = Fixture::new();
        identity::set_project_role(
            &f.work.db,
            &f.admin,
            "alpha",
            &f.bob.user_id,
            Some(ProjectRole::Contributor),
            5,
        )
        .unwrap();
        let root = f
            .work
            .submit(&f.alice, &request("handover", "parent", Some(1000)), 10)
            .unwrap();
        let parent = f.work.claim("WinC", 11).unwrap().unwrap();
        report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
        report_at(
            &mut f,
            &parent,
            "yield",
            ReportOutcome::YieldToChild {
                checkpoint: json!({"version":1}),
                child_environment_id: "solver".into(),
                child_title: "uncertain child".into(),
                child_input: json!({"prompt":"child"}),
                resources_released: true,
            },
            20,
        )
        .unwrap();
        let child = f.work.claim("WinE", 30).unwrap().unwrap();
        report_at(
            &mut f,
            &child,
            "uncertain",
            ReportOutcome::Uncertain {
                reason: "preparation could not be reconciled".into(),
            },
            40,
        )
        .unwrap();
        let latest = f.work.job(&f.alice, &root.id).unwrap();
        f.work
            .handover(
                &f.alice,
                &root.id,
                &HandoverRequest {
                    expected_revision: latest.revision,
                    new_assignee_id: f.bob.user_id.clone(),
                },
                50,
            )
            .unwrap();
        let held = f.work.assignments("WinE").unwrap();
        assert_eq!(
            held.len(),
            1,
            "handover must not release uncertain preparation capacity"
        );
        assert!(held[0].stop_requested);
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().assignee_id,
            f.alice.user_id
        );
        assert!(
            f.work
                .handover_view(&f.alice, &root.id)
                .unwrap()
                .pending
                .is_some()
        );
        let now = if expired { 1001 } else { 80 };
        f.work.expire_start_deadlines(now).unwrap();
        assert!(report_at(&mut f, &child, "late-start", ReportOutcome::Started, now).is_err());
        assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
        if manual {
            let command = ManagementCommand {
                request_id: "confirmed-stop".into(),
                expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
                    .as_str()
                    .unwrap()
                    .into(),
                request: ManagementRequest::SettleAttempt {
                    attempt_id: child.attempt_id.clone(),
                    generation: child.generation,
                    acknowledge_stopped: true,
                    reason: "operator confirmed the process has stopped".into(),
                },
            };
            f.work.manage(&f.admin, &command, now).unwrap();
            f.work.manage(&f.admin, &command, now + 1).unwrap();
        } else {
            report_at(
                &mut f,
                &child,
                "stopped",
                ReportOutcome::Finished {
                    success: false,
                    result: json!({"stopped":true}),
                    resources_released: true,
                },
                now,
            )
            .unwrap();
        }
        assert!(f.work.assignments("WinE").unwrap().is_empty());
        let final_root = f.work.job(&f.alice, &root.id).unwrap();
        if expired {
            assert_eq!(final_root.state, JobState::Failed);
            assert_eq!(final_root.assignee_id, f.alice.user_id);
        } else {
            assert_eq!(final_root.state, JobState::Queued);
            assert_eq!(final_root.assignee_id, f.bob.user_id);
        }
        assert!(
            f.work
                .handover_view(&f.alice, &root.id)
                .unwrap()
                .pending
                .is_none()
        );
        let replay = report_at(
            &mut f,
            &child,
            "stopped",
            ReportOutcome::Finished {
                success: false,
                result: json!({"stopped":true}),
                resources_released: true,
            },
            now + 1,
        );
        if manual {
            assert!(replay.is_err());
        } else {
            replay.unwrap();
        }
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().revision,
            final_root.revision
        );
        assert!(f.work.assignments("WinE").unwrap().is_empty());
    }
}
