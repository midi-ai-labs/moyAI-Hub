use super::*;

fn edit_overview(f: &Fixture, overview: &str) {
    f.work
        .db
        .execute(
            "UPDATE shared_projects SET overview=?1 WHERE id='alpha'",
            [overview],
        )
        .unwrap();
    f.work
        .db
        .execute(
            "UPDATE shared_management_state SET revision=revision+1 WHERE id=1",
            [],
        )
        .unwrap();
}

#[test]
fn accepted_context_survives_child_redelegation_parent_resume_and_hub_restart() {
    let mut f = Fixture::new();
    edit_overview(&f, "solver: Worker\nparent: API");
    let root = f.submit("parent", "overview-root", 2);
    let first = f.work.claim("WinC", 11).unwrap().unwrap();
    let context = serde_json::to_value(first.project_context.as_ref().unwrap()).unwrap();
    assert_eq!(context["project_id"], "alpha");
    assert_eq!(context["root_prompt"], "test");
    assert_eq!(context["overview"], "solver: Worker\nparent: API");
    f.report(&first, "context-start", ReportOutcome::Started);
    f.report(&first, "context-yield", child());
    edit_overview(&f, "NEW description for future requests");
    f.reopen();
    let child_attempt = f.work.claim("WinE", 21).unwrap().unwrap();
    assert_eq!(
        child_attempt.job.parent_id.as_deref(),
        Some(root.id.as_str())
    );
    assert_eq!(
        serde_json::to_value(&child_attempt.project_context).unwrap(),
        context
    );
    assert_eq!(child_attempt.job.environment_id, "solver");
    f.report(
        &child_attempt,
        "context-child-start",
        ReportOutcome::Started,
    );
    f.report(
        &child_attempt,
        "context-child-yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"next":"consume-grandchild"}),
            child_environment_id: "parent".into(),
            child_title: "Local verification".into(),
            child_input: json!({"prompt":"Verify the agreed connection"}),
            resources_released: true,
        },
    );
    let grandchild = f.work.claim("WinC", 22).unwrap().unwrap();
    assert_eq!(
        grandchild.job.parent_id.as_deref(),
        Some(child_attempt.job.id.as_str())
    );
    assert_eq!(
        serde_json::to_value(&grandchild.project_context).unwrap(),
        context
    );
    f.report(
        &grandchild,
        "context-grandchild-start",
        ReportOutcome::Started,
    );
    f.report(&grandchild, "context-grandchild-done", finished());
    let child_resumed = f.work.claim("WinE", 23).unwrap().unwrap();
    assert_eq!(child_resumed.job.id, child_attempt.job.id);
    assert_eq!(
        serde_json::to_value(&child_resumed.project_context).unwrap(),
        context
    );
    f.report(
        &child_resumed,
        "context-child-resume-start",
        ReportOutcome::Started,
    );
    f.report(&child_resumed, "context-child-done", finished());
    let resumed = f.work.claim("WinC", 25).unwrap().unwrap();
    assert_eq!(resumed.job.id, root.id);
    assert_eq!(
        serde_json::to_value(&resumed.project_context).unwrap(),
        context
    );
    assert!(resumed.child_result.is_some());
    f.report(&resumed, "context-resume-start", ReportOutcome::Started);
    f.report(&resumed, "context-root-done", finished());
    let next = f.submit("parent", "new-overview-root", 0);
    let next_assignment = f.work.claim("WinC", 30).unwrap().unwrap();
    assert_eq!(next_assignment.job.id, next.id);
    let next_context = next_assignment.project_context.unwrap();
    assert_eq!(next_context.overview, "NEW description for future requests");
    assert_ne!(next_context.revision, context["revision"].as_str().unwrap());
}

#[test]
fn accepted_overview_does_not_preserve_revoked_execution_authority() {
    let mut f = Fixture::new();
    edit_overview(&f, "The project description is not permission to execute.");
    f.submit("parent", "context-before-revocation", 0);
    let assignment = f.work.claim("WinC", 11).unwrap().unwrap();
    assert_eq!(
        assignment.project_context.as_ref().unwrap().overview,
        "The project description is not permission to execute."
    );
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 12).unwrap();
    let started = Report {
        event_id: "context-after-revocation".into(),
        attempt_id: assignment.attempt_id.clone(),
        generation: assignment.generation,
        outcome: ReportOutcome::Started,
    };
    assert!(f.work.report("WinC", &started, 13).is_err());
    assert_ne!(
        f.work
            .attempt("WinC", &assignment.attempt_id)
            .unwrap()
            .state,
        "running"
    );
}

#[test]
fn overview_edit_uses_current_admin_revision_and_preserves_legacy_omission() {
    let mut f = Fixture::new();
    let mut command = ManagementCommand {
        request_id: "overview-edit".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: "described".into(),
            label: "解析環境".into(),
            overview: Some("  WinB: Worker\n  WinC: DB \n".into()),
            archived: false,
            members: None,
            controller_device_ids: vec![],
            runner_device_ids: vec![],
        },
    };
    assert!(f.work.manage(&f.alice, &command, 40).is_err());
    let saved = f.work.manage(&f.admin, &command, 40).unwrap();
    let overview = |snapshot: &Value| {
        snapshot["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "described")
            .unwrap()["overview"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(overview(&saved), "WinB: Worker\n  WinC: DB");
    command.request_id = "stale-overview-edit".into();
    assert!(matches!(
        f.work.manage(&f.admin, &command, 41),
        Err(WorkError::Conflict)
    ));
    command.expected_revision = saved["revision"].as_str().unwrap().into();
    if let ManagementRequest::SaveProject { overview, .. } = &mut command.request {
        *overview = None;
    }
    let preserved = f.work.manage(&f.admin, &command, 42).unwrap();
    assert_eq!(overview(&preserved), overview(&saved));
    f.reopen();
    assert_eq!(
        overview(&f.work.management_snapshot(&f.admin).unwrap()),
        overview(&saved)
    );
    command.request_id = "oversized-overview".into();
    command.expected_revision = preserved["revision"].as_str().unwrap().into();
    if let ManagementRequest::SaveProject { overview, .. } = &mut command.request {
        *overview = Some("あ".repeat(2731));
    }
    assert!(matches!(
        f.work.manage(&f.admin, &command, 43),
        Err(WorkError::Management(_))
    ));
    assert_eq!(
        overview(&f.work.management_snapshot(&f.admin).unwrap()),
        overview(&saved)
    );
    command.request_id = "overview-byte-boundary".into();
    if let ManagementRequest::SaveProject { overview, .. } = &mut command.request {
        *overview = Some(format!("{}ab", "あ".repeat(2730)));
    }
    let boundary = f.work.manage(&f.admin, &command, 44).unwrap();
    assert_eq!(overview(&boundary).len(), 8192);
}

#[test]
fn v23_upgrade_preserves_legacy_jobs_without_inventing_past_overviews() {
    let mut f = Fixture::new();
    let existing = f.submit("parent", "pre-context-job", 0);
    f.work.db.execute_batch("ALTER TABLE shared_projects DROP COLUMN overview; ALTER TABLE shared_jobs DROP COLUMN project_context_json; PRAGMA user_version=23;").unwrap();
    f.reopen();
    let snapshot = f.work.management_snapshot(&f.admin).unwrap();
    assert!(
        snapshot["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["overview"] == "")
    );
    let assignment = f.work.claim("WinC", 21).unwrap().unwrap();
    assert_eq!(assignment.job.id, existing.id);
    assert!(assignment.project_context.is_none());
}
