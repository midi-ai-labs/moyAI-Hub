//! Observed shared-control communication, never execution or capacity authority.
use super::*;
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

pub const RUNNER_CONTACT_STALE_MS: u64 = 15_000;
const ENVIRONMENT_OBSERVATIONS: usize = 1024;
const ATTEMPT_OBSERVATIONS: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerContactState {
    Unconfirmed,
    Recent,
    Stale,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerContact {
    pub last_contact_ms: Option<u64>,
    pub state: RunnerContactState,
}

#[derive(Default)]
pub(super) struct Observations {
    environments: BTreeMap<(String, String), u64>,
    attempts: BTreeMap<(String, String), u64>,
}

fn record(
    map: &mut BTreeMap<(String, String), u64>,
    runner: &str,
    id: &str,
    now: u64,
    limit: usize,
) {
    let key = (runner.to_owned(), id.to_owned());
    if !map.contains_key(&key) && map.len() >= limit {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, time)| **time)
            .map(|(key, _)| key.clone())
        {
            map.remove(&oldest);
        }
    }
    map.insert(key, now);
}
fn projection(last: Option<u64>, now: u64) -> RunnerContact {
    let state = match last {
        None => RunnerContactState::Unconfirmed,
        Some(last) if now >= last && now - last < RUNNER_CONTACT_STALE_MS => {
            RunnerContactState::Recent
        }
        Some(_) => RunnerContactState::Stale,
    };
    RunnerContact {
        last_contact_ms: last,
        state,
    }
}

impl SharedWork {
    pub(super) fn owned_contact_scope(
        &self,
        runner: &str,
        scope: &[String],
    ) -> Result<Vec<String>> {
        store::id(runner)?;
        if scope.len() > 128 {
            return Err(WorkError::Invalid);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut owned = vec![];
        for environment in scope {
            store::id(environment)?;
            if !seen.insert(environment) {
                return Err(WorkError::Invalid);
            }
            let owner: Option<String> = self
                .db
                .query_row(
                    "SELECT runner_id FROM shared_environments WHERE id=?1",
                    [environment],
                    |r| r.get(0),
                )
                .optional()?;
            if owner.as_deref() == Some(runner) {
                owned.push(environment.clone());
            }
        }
        Ok(owned)
    }
    fn record_environment_scope(&mut self, runner: &str, scope: &[String], now: u64) {
        for environment in scope {
            record(
                &mut self.runner_contacts.environments,
                runner,
                environment,
                now,
                ENVIRONMENT_OBSERVATIONS,
            );
        }
    }
    pub(super) fn observe_attempt(&mut self, runner: &str, attempt: &str, now: u64) -> Result<()> {
        store::timestamp(now)?;
        let row: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT runner_id,state FROM shared_attempts WHERE id=?1",
                [attempt],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((owner, state)) = row else {
            return Err(WorkError::Forbidden);
        };
        if owner != runner {
            return Err(WorkError::Forbidden);
        }
        if !matches!(state.as_str(), "assigned" | "running" | "stop_requested") {
            self.runner_contacts
                .attempts
                .remove(&(runner.into(), attempt.into()));
            return Ok(());
        }
        record(
            &mut self.runner_contacts.attempts,
            runner,
            attempt,
            now,
            ATTEMPT_OBSERVATIONS,
        );
        Ok(())
    }
    pub(super) fn claim_observed(
        &mut self,
        runner: &str,
        scope: Option<&[String]>,
        now: u64,
    ) -> Result<Option<Assignment>> {
        let owned = scope
            .map(|scope| self.owned_contact_scope(runner, scope))
            .transpose()?;
        let assignment = self.claim_for(runner, scope, now)?;
        if let Some(scope) = &owned {
            self.record_environment_scope(runner, scope, now);
        }
        if let Some(assignment) = &assignment {
            self.observe_attempt(runner, &assignment.attempt_id, now)?;
        }
        Ok(assignment)
    }
    pub(super) fn assignments_observed(
        &mut self,
        runner: &str,
        scope: Option<&[String]>,
        now: u64,
    ) -> Result<Vec<Assignment>> {
        store::timestamp(now)?;
        let owned = scope
            .map(|scope| self.owned_contact_scope(runner, scope))
            .transpose()?;
        let mut assignments = self.assignments(runner)?;
        if let Some(scope) = &owned {
            assignments.retain(|assignment| scope.contains(&assignment.job.environment_id));
            self.record_environment_scope(runner, scope, now);
            for assignment in &assignments {
                self.observe_attempt(runner, &assignment.attempt_id, now)?;
            }
        }
        // Legacy unscoped polling still receives its existing assignments. It
        // cannot assert that every configuration on this device is responding.
        Ok(assignments)
    }
    pub(super) fn environment_contact(
        &self,
        runner: &str,
        environment: &str,
        now: u64,
    ) -> RunnerContact {
        projection(
            self.runner_contacts
                .environments
                .get(&(runner.into(), environment.into()))
                .copied(),
            now,
        )
    }
    pub(super) fn job_contact(&self, job: &Job, now: u64) -> Result<RunnerContact> {
        let attempt:Option<(String,String)>=self.db.query_row("SELECT id,runner_id FROM shared_attempts WHERE job_id=?1 AND state IN ('assigned','running','stop_requested')",[&job.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((id, runner)) = attempt {
            return Ok(projection(
                self.runner_contacts.attempts.get(&(runner, id)).copied(),
                now,
            ));
        }
        let runner: String = self.db.query_row(
            "SELECT runner_id FROM shared_environments WHERE id=?1",
            [&job.environment_id],
            |r| r.get(0),
        )?;
        Ok(self.environment_contact(&runner, &job.environment_id, now))
    }
}
