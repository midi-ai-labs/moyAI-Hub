use super::*;

fn running(f: &mut Fixture, environment: &str, key: &str, runner: &str) -> Assignment {
    f.submit(environment, key, 0);
    let attempt = f
        .work
        .claim_for(runner, Some(&[environment.into()]), 11)
        .unwrap()
        .unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    attempt
}
fn contact(f: &Fixture, job: &str, now: u64) -> Value {
    f.work.job_detail_at(&f.alice, job, now).unwrap()["runner_contact"].clone()
}

#[test]
fn runner_contact_stale_restart_and_desktop_reads_preserve_running_authority_and_capacity() {
    let mut f = Fixture::new();
    let attempt = running(&mut f, "solver", "active", "WinE");
    let original = f.work.job(&f.alice, &attempt.job.id).unwrap();
    assert_eq!(contact(&f, &original.id, 100)["state"], "unconfirmed");
    f.work
        .assignments_observed("WinE", Some(&["solver".into()]), 100)
        .unwrap();
    for now in [100, 100 + RUNNER_CONTACT_STALE_MS - 1] {
        let status = f
            .work
            .status_at(&f.alice, "alpha", None, 100, None, now)
            .unwrap();
        assert_eq!(
            status.jobs[0].runner_contact.state,
            RunnerContactState::Recent
        );
        let env = status
            .environments
            .iter()
            .find(|e| e.id == "solver")
            .unwrap();
        assert_eq!(env.runner_contact.state, RunnerContactState::Recent);
        assert_eq!(env.occupied, 1);
        assert_eq!(contact(&f, &original.id, now)["last_contact_ms"], 100);
    }
    let stale = 100 + RUNNER_CONTACT_STALE_MS;
    for now in [stale, stale + 1000] {
        let detail = f.work.job_detail_at(&f.alice, &original.id, now).unwrap();
        assert_eq!(detail["runner_contact"]["state"], "stale");
        assert!(detail["wait_reason"].as_str().unwrap().contains("通信"));
        assert_eq!(detail["state"], "running");
        assert_eq!(detail["revision"], original.revision);
        assert!(detail["result"].is_null());
        assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
    }
    f.work
        .assignments_observed("WinE", Some(&["solver".into()]), stale + 1001)
        .unwrap();
    assert_eq!(contact(&f, &original.id, stale + 1001)["state"], "recent");
    assert_eq!(contact(&f, &original.id, 99)["state"], "stale");
    f.reopen();
    assert_eq!(
        contact(&f, &original.id, stale + 1002),
        json!({"state":"unconfirmed","last_contact_ms":null})
    );
    assert_eq!(
        f.work.job(&f.alice, &original.id).unwrap().state,
        JobState::Running
    );
    assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
    let status = f
        .work
        .status_at(&f.alice, "alpha", None, 100, None, stale + 1002)
        .unwrap();
    assert!(
        status
            .environments
            .iter()
            .all(|env| env.runner_contact.state == RunnerContactState::Unconfirmed)
    );
    assert_eq!(
        status
            .environments
            .iter()
            .find(|env| env.id == "parent")
            .unwrap()
            .occupied,
        0
    );
}

#[test]
fn runner_contact_scope_rejects_foreign_environments_and_isolates_same_device_configurations() {
    let mut f = Fixture::new();
    f.work
        .register_environment(
            &f.admin,
            &Environment {
                id: "other-config".into(),
                label: "Other".into(),
                resource_id: "other-slot".into(),
                runner_id: "WinE".into(),
                capacity: 1,
                project_ids: vec!["alpha".into()],
            },
        )
        .unwrap();
    let first = running(&mut f, "solver", "first", "WinE");
    let second = running(&mut f, "other-config", "second", "WinE");
    let selected = f
        .work
        .assignments_observed("WinE", Some(&["solver".into()]), 100)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].attempt_id, first.attempt_id);
    assert_eq!(contact(&f, &first.job.id, 100)["state"], "recent");
    assert_eq!(contact(&f, &second.job.id, 100)["state"], "unconfirmed");
    assert_eq!(
        f.work
            .assignments_observed("WinE", None, 110)
            .unwrap()
            .len(),
        2
    );
    assert!(
        f.work
            .assignments_observed("WinE", Some(&[]), 111)
            .unwrap()
            .is_empty()
    );
    assert_eq!(contact(&f, &first.job.id, 111)["last_contact_ms"], 100);
    assert_eq!(contact(&f, &second.job.id, 111)["state"], "unconfirmed");
    for scope in [
        vec!["solver".into(), "solver".into()],
        vec!["bad,part".into()],
    ] {
        assert!(
            f.work
                .assignments_observed("WinE", Some(&scope), 120)
                .is_err()
        );
        assert!(f.work.claim_observed("WinE", Some(&scope), 120).is_err());
    }
    for scope in [vec!["parent".into()], vec!["missing".into()]] {
        assert!(
            f.work
                .assignments_observed("WinE", Some(&scope), 120)
                .unwrap()
                .is_empty()
        );
        assert!(
            f.work
                .claim_observed("WinE", Some(&scope), 120)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        f.work.environment_contact("WinC", "parent", 120).state,
        RunnerContactState::Unconfirmed
    );
    assert_eq!(contact(&f, &first.job.id, 120)["last_contact_ms"], 100);
    assert!(
        f.work
            .observe_attempt("WinC", &second.attempt_id, 130)
            .is_err()
    );
    f.work
        .observe_attempt("WinE", &second.attempt_id, 130)
        .unwrap();
    assert_eq!(contact(&f, &second.job.id, 130)["state"], "recent");
    assert_eq!(contact(&f, &first.job.id, 130)["last_contact_ms"], 100);
    assert_eq!(
        f.work
            .environment_contact("WinE", "other-config", 130)
            .state,
        RunnerContactState::Unconfirmed
    );
    f.work.cancel(&f.alice, &first.job.id, 140).unwrap();
    let mixed = f
        .work
        .assignments_observed(
            "WinE",
            Some(&["solver".into(), "missing".into(), "parent".into()]),
            141,
        )
        .unwrap();
    assert_eq!(mixed.len(), 1);
    assert_eq!(mixed[0].attempt_id, first.attempt_id);
    assert!(mixed[0].stop_requested);
    assert_eq!(
        f.work.environment_contact("WinC", "parent", 141).state,
        RunnerContactState::Unconfirmed
    );
}

#[test]
fn runner_contact_idle_claim_confirms_only_explicit_scope_and_terminal_reports_retire_observation()
{
    let mut f = Fixture::new();
    assert!(f.work.claim_observed("WinC", None, 100).unwrap().is_none());
    assert_eq!(
        f.work.environment_contact("WinC", "parent", 100).state,
        RunnerContactState::Unconfirmed
    );
    assert!(
        f.work
            .claim_observed("WinC", Some(&["parent".into()]), 101)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.work.environment_contact("WinC", "parent", 101).state,
        RunnerContactState::Recent
    );
    let attempt = running(&mut f, "solver", "terminal", "WinE");
    f.work
        .observe_attempt("WinE", &attempt.attempt_id, 102)
        .unwrap();
    assert_eq!(contact(&f, &attempt.job.id, 102)["state"], "recent");
    f.report(&attempt, "finished", finished());
    f.work
        .observe_attempt("WinE", &attempt.attempt_id, 103)
        .unwrap();
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    assert_eq!(
        f.work.job(&f.alice, &attempt.job.id).unwrap().state,
        JobState::Succeeded
    );
    assert_eq!(contact(&f, &attempt.job.id, 103)["state"], "unconfirmed");
}
