use super::*;

fn fill_terminal_history(f: &Fixture, source: &str, total: u64) {
    let retained: u64 = f
        .work
        .db
        .query_row("SELECT COUNT(*) FROM shared_jobs", [], |row| row.get(0))
        .unwrap();
    assert!(total > retained);
    f.work
        .db
        .execute(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?2)
             INSERT INTO shared_jobs(id,root_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms)
             SELECT 'retained-'||x,'retained-'||x,project_id,environment_id,requestor_id,assignee_id,'retained-request-'||x,'{}','retained','{}','succeeded',0,1,1,1
             FROM n,shared_jobs WHERE id=?1",
            rusqlite::params![source, total - retained],
        )
        .unwrap();
}

#[test]
fn terminal_children_release_slots_across_twelve_resumptions_and_reopen() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "reusable-budget", 1);
    let schema_version: u32 = f
        .work
        .db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let mut parent = f.work.claim("WinC", 12).unwrap().unwrap();
    for iteration in 0..12 {
        f.report(&parent, "start", ReportOutcome::Started);
        let waiting = f.report(&parent, "yield", child());
        assert_eq!(waiting.state, JobState::WaitingChild);
        let child = f.work.claim("WinE", 30 + iteration * 2).unwrap().unwrap();
        assert_eq!(
            waiting.awaiting_child_id.as_deref(),
            Some(child.job.id.as_str())
        );
        f.report(&child, "start", ReportOutcome::Started);
        let success = iteration != 5;
        let result = f.report(
            &child,
            "done",
            ReportOutcome::Finished {
                success,
                result: json!({"iteration": iteration}),
                resources_released: true,
            },
        );
        assert_eq!(
            result.state,
            if success {
                JobState::Succeeded
            } else {
                JobState::Failed
            }
        );
        // Existing stored budgets and terminal children survive a normal reopen.
        if iteration == 0 {
            f.reopen();
            assert_eq!(
                f.work
                    .db
                    .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                    .unwrap(),
                schema_version
            );
            assert_eq!(
                f.work
                    .db
                    .query_row(
                        "SELECT descendant_budget FROM shared_jobs WHERE id=?1",
                        [&root.id],
                        |row| row.get::<_, u32>(0)
                    )
                    .unwrap(),
                1
            );
        }
        parent = f.work.claim("WinC", 31 + iteration * 2).unwrap().unwrap();
        assert_eq!(parent.job.id, root.id);
        assert_eq!(parent.child_result.as_ref().unwrap().id, child.job.id);
        assert_eq!(parent.child_result.as_ref().unwrap().state, result.state);
    }
    assert_eq!(parent.generation, 13);
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM shared_jobs WHERE root_id=?1 AND parent_id IS NOT NULL",
                [&root.id],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        12
    );
    f.report(&parent, "start", ReportOutcome::Started);
    assert_eq!(
        f.report(&parent, "done", finished()).state,
        JobState::Succeeded
    );
}

#[test]
fn zero_budget_still_disallows_the_first_child() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "no-children", 0);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    let before = f.report(&parent, "start", ReportOutcome::Started);
    assert!(matches!(
        f.work.report(
            "WinC",
            &Report {
                event_id: "forbidden-child".into(),
                attempt_id: parent.attempt_id,
                generation: parent.generation,
                outcome: child(),
            },
            21
        ),
        Err(WorkError::Capacity)
    ));
    let after = f.work.job(&f.alice, &root.id).unwrap();
    assert_eq!(after.state, JobState::Running);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.checkpoint, before.checkpoint);
    assert_eq!(after.awaiting_child_id, before.awaiting_child_id);
}

#[test]
fn nested_waiting_children_count_toward_the_root_limit() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "nested", 2);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let intermediate = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&intermediate, "start", ReportOutcome::Started);
    f.report(
        &intermediate,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"next":"consume-grandchild"}),
            child_environment_id: "parent".into(),
            child_title: "grandchild".into(),
            child_input: json!({}),
            resources_released: true,
        },
    );
    let grandchild = f.work.claim("WinC", 22).unwrap().unwrap();
    assert_eq!(
        grandchild.job.parent_id.as_deref(),
        Some(intermediate.job.id.as_str())
    );
    assert_eq!(
        f.work.job(&f.alice, &intermediate.job.id).unwrap().state,
        JobState::WaitingChild
    );
    f.report(&grandchild, "start", ReportOutcome::Started);
    assert!(matches!(
        f.work.report(
            "WinC",
            &Report {
                event_id: "too-deep".into(),
                attempt_id: grandchild.attempt_id.clone(),
                generation: grandchild.generation,
                outcome: child(),
            },
            23
        ),
        Err(WorkError::Capacity)
    ));
    f.report(&grandchild, "done", finished());
    let resumed_child = f.work.claim("WinE", 24).unwrap().unwrap();
    assert_eq!(resumed_child.job.id, intermediate.job.id);
    f.report(&resumed_child, "start", ReportOutcome::Started);
    // One waiting child remains; the completed grandchild has returned its slot.
    assert_eq!(
        f.report(&resumed_child, "next-child", child()).state,
        JobState::WaitingChild
    );
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::WaitingChild
    );
}

#[test]
fn cancelling_children_remain_in_the_open_slot_reservation() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "cancelling", 1);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&child, "start", ReportOutcome::Started);
    f.work.cancel(&f.alice, &root.id, 22).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelling
    );
    assert_eq!(
        f.work.job(&f.alice, &child.job.id).unwrap().state,
        JobState::Cancelling
    );
    fill_terminal_history(&f, &root.id, 99_999);
    // The existing cancelling child still occupies the reserved child slot;
    // there is no second uncreated child slot to reserve before its stop ACK.
    assert!(store::root_capacity(&f.work.db, 0).is_ok());
    assert!(matches!(
        store::root_capacity(&f.work.db, 1),
        Err(WorkError::Capacity)
    ));
    f.report(&child, "stopped", finished());
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        f.work.job(&f.alice, &child.job.id).unwrap().state,
        JobState::Cancelled
    );
}

#[test]
fn history_last_slot_is_atomic_and_exact_report_replay_still_succeeds() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "last-child-slot", 2);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    fill_terminal_history(&f, &root.id, 99_999);
    let report = Report {
        event_id: "last-slot".into(),
        attempt_id: parent.attempt_id.clone(),
        generation: parent.generation,
        outcome: child(),
    };
    let waiting = f.work.report("WinC", &report, 21).unwrap();
    let replay = f.work.report("WinC", &report, 22).unwrap();
    assert_eq!(replay.awaiting_child_id, waiting.awaiting_child_id);
    assert_eq!(replay.revision, waiting.revision);
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_jobs", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        100_000
    );
    let child = f.work.claim("WinE", 23).unwrap().unwrap();
    f.report(&child, "start", ReportOutcome::Started);
    f.report(&child, "done", finished());
    let resumed = f.work.claim("WinC", 24).unwrap().unwrap();
    let before = f.report(&resumed, "start", ReportOutcome::Started);
    let rejected = Report {
        event_id: "storage-full".into(),
        attempt_id: resumed.attempt_id,
        generation: resumed.generation,
        outcome: super::child(),
    };
    assert!(matches!(
        f.work.report("WinC", &rejected, 25),
        Err(WorkError::Capacity)
    ));
    let after = f.work.job(&f.alice, &root.id).unwrap();
    assert_eq!(after.state, JobState::Running);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.checkpoint, before.checkpoint);
    assert_eq!(after.awaiting_child_id, before.awaiting_child_id);
    assert_eq!(
        f.work
            .db
            .query_row("SELECT COUNT(*) FROM shared_jobs", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        100_000
    );
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM shared_reports WHERE attempt_id=?1 AND event_id=?2",
                rusqlite::params![rejected.attempt_id, rejected.event_id],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert!(f.work.report("WinC", &report, 26).is_ok());
}

#[test]
fn root_admission_reserves_reusable_slots_after_terminal_history() {
    let mut f = Fixture::new();
    let root = f.submit("parent", "open-reservation", 1);
    let parent = f.work.claim("WinC", 12).unwrap().unwrap();
    f.report(&parent, "start", ReportOutcome::Started);
    f.report(&parent, "yield", child());
    let child = f.work.claim("WinE", 21).unwrap().unwrap();
    f.report(&child, "start", ReportOutcome::Started);
    f.report(&child, "done", finished());
    f.reopen();
    fill_terminal_history(&f, &root.id, 99_998);
    assert!(store::root_capacity(&f.work.db, 0).is_ok());
    // The existing queued root can use its child slot again, so a new root
    // needing another child would overbook the remaining history capacity.
    assert!(matches!(
        store::root_capacity(&f.work.db, 1),
        Err(WorkError::Capacity)
    ));
}
