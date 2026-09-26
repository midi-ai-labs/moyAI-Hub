use super::*;

fn restricted() -> Fixture {
    let mut f = Fixture::new();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "beta",
        &f.alice.user_id,
        Some(ProjectRole::Contributor),
        2,
    )
    .unwrap();
    for (project, controller) in [("alpha", "WinA"), ("beta", "WinB")] {
        let mut members = vec![ProjectMember {
            user_id: f.alice.user_id.clone(),
            role: ProjectRole::Contributor,
        }];
        if project == "beta" {
            members.push(ProjectMember {
                user_id: f.bob.user_id.clone(),
                role: ProjectRole::Contributor,
            });
        }
        let command = ManagementCommand {
            request_id: format!("setup-{project}"),
            expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
                .as_str()
                .unwrap()
                .into(),
            request: ManagementRequest::SaveProject {
                id: project.into(),
                label: project.into(),
                archived: false,
                members: Some(members),
                controller_device_ids: vec![controller.into()],
                runner_device_ids: vec!["WinE".into()],
            },
        };
        f.work.manage(&f.admin, &command, 3).unwrap();
    }
    f
}
fn beta(f: &mut Fixture, key: &str) -> Job {
    f.work
        .submit(
            &f.alice,
            &SubmitJob {
                request_id: key.into(),
                project_id: "beta".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "solver".into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "Beta private title".into(),
                input: json!({}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            10,
        )
        .unwrap()
}

#[test]
fn controller_projection_keeps_shared_occupancy_without_disclosing_other_pc_project() {
    let mut f = restricted();
    let job = beta(&mut f, "beta-occupied");
    f.work.claim("WinE", 20).unwrap().unwrap();
    let visible = f.work.status(&f.alice, "alpha", None, 10, None).unwrap();
    assert_eq!(
        visible
            .environments
            .iter()
            .find(|e| e.id == "solver")
            .unwrap()
            .occupants[0]
            .job_id,
        job.id
    );
    let filtered = f
        .work
        .status_for_device(&f.alice, "alpha", None, 10, None, Some("WinA"))
        .unwrap();
    let solver = filtered
        .environments
        .iter()
        .find(|e| e.id == "solver")
        .unwrap();
    assert_eq!(
        (
            solver.occupied,
            solver.other_occupants,
            solver.additional_visible_occupants
        ),
        (1, 1, 0)
    );
    assert!(solver.occupants.is_empty());
    assert!(
        f.work
            .status_for_device(&f.alice, "beta", None, 10, None, Some("WinA"))
            .is_err()
    );
    assert!(
        f.work
            .status_for_device(&f.bob, "alpha", None, 10, None, Some("WinA"))
            .is_err()
    );
}

#[test]
fn controller_inbox_filters_unread_and_pages_before_limit_and_rejects_hidden_read() {
    let mut f = restricted();
    let a = f.submit("solver", "alpha-one", 0);
    f.work.cancel(&f.alice, &a.id, 20).unwrap();
    let b = beta(&mut f, "beta-secret");
    f.work.cancel(&f.alice, &b.id, 30).unwrap();
    let c = f.submit("solver", "alpha-two", 0);
    f.work.cancel(&f.alice, &c.id, 40).unwrap();
    let all = f.work.inbox(&f.alice, None, 10, 50).unwrap();
    assert_eq!(all["unread_count"], 3);
    let secret = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["job_id"] == b.id)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let first = f
        .work
        .inbox_for_device(&f.alice, None, 1, 50, Some("WinA"))
        .unwrap();
    assert_eq!(first["unread_count"], 2);
    assert_eq!(first["items"][0]["job_id"], c.id);
    let second = f
        .work
        .inbox_for_device(&f.alice, first["next_before"].as_str(), 1, 50, Some("WinA"))
        .unwrap();
    assert_eq!(second["items"][0]["job_id"], a.id);
    assert!(second["next_before"].is_null());
    assert!(
        f.work
            .read_inbox_for_device(&f.alice, secret, 51, Some("WinA"))
            .is_err()
    );
    assert_eq!(
        f.work
            .inbox_for_device(&f.alice, None, 10, 52, Some("WinB"))
            .unwrap()["unread_count"],
        1
    );
    f.work
        .read_inbox_for_device(
            &f.alice,
            first["items"][0]["id"].as_str().unwrap(),
            53,
            Some("WinA"),
        )
        .unwrap();
    assert_eq!(
        f.work
            .inbox_for_device(&f.alice, None, 10, 54, Some("WinA"))
            .unwrap()["unread_count"],
        1
    );
}
