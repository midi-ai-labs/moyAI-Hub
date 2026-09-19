//! Durable template requests. The Runner owns paths; Hub owns project/resource associations.
use super::*;
use crate::identity::{self, Principal};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

pub(super) const MAX_RUNNER_ENVIRONMENTS: u32 = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerTemplate {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProvisionRequest {
    pub environment_id: String,
    pub template_id: String,
    #[serde(default)]
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionReceipt {
    pub environment_id: String,
    pub template_id: String,
    pub generation: u64,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunnerEnvironment {
    #[serde(flatten)]
    pub environment: Environment,
    pub enabled: bool,
}

impl SharedWork {
    pub fn runner_environments(&self, runner: &str) -> Result<Vec<RunnerEnvironment>> {
        store::id(runner)?;
        let mut statement=self.db.prepare("SELECT e.id,e.label,e.resource_id,r.capacity,e.enabled FROM shared_environments e JOIN shared_resources r ON r.id=e.resource_id WHERE e.runner_id=?1 ORDER BY e.id LIMIT ?2")?;
        let mut environments = statement
            .query_map(params![runner, MAX_RUNNER_ENVIRONMENTS + 1], |row| {
                Ok(RunnerEnvironment {
                    environment: Environment {
                        id: row.get(0)?,
                        label: row.get(1)?,
                        resource_id: row.get(2)?,
                        runner_id: runner.into(),
                        capacity: row.get(3)?,
                        project_ids: vec![],
                    },
                    enabled: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if environments.len() > MAX_RUNNER_ENVIRONMENTS as usize {
            return Err(WorkError::Capacity);
        }
        for environment in &mut environments {
            let mut projects=self.db.prepare("SELECT project_id FROM shared_environment_projects WHERE environment_id=?1 ORDER BY project_id LIMIT 1000")?;
            environment.environment.project_ids = projects
                .query_map([&environment.environment.id], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
        Ok(environments)
    }
    pub fn publish_templates(
        &mut self,
        runner: &str,
        templates: &[RunnerTemplate],
        now: u64,
    ) -> Result<()> {
        store::id(runner)?;
        store::timestamp(now)?;
        if templates.len() > 128 {
            return Err(WorkError::Invalid);
        }
        let mut ids = std::collections::BTreeSet::new();
        for template in templates {
            store::id(&template.id)?;
            store::label(&template.label)?;
            if !ids.insert(&template.id) {
                return Err(WorkError::Invalid);
            }
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = {
            let mut statement = tx.prepare("SELECT template_id,label FROM shared_runner_templates WHERE runner_id=?1 ORDER BY template_id")?;
            statement
                .query_map([runner], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut proposed = templates
            .iter()
            .map(|value| (value.id.clone(), value.label.clone()))
            .collect::<Vec<_>>();
        proposed.sort();
        if current == proposed {
            super::project_setup::synchronize(&tx, Some(runner), now)?;
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "DELETE FROM shared_runner_templates WHERE runner_id=?1",
            [runner],
        )?;
        for template in templates {
            tx.execute(
                "INSERT INTO shared_runner_templates VALUES(?1,?2,?3,?4)",
                params![runner, template.id, template.label, now],
            )?;
        }
        super::project_setup::synchronize(&tx, Some(runner), now)?;
        tx.commit()?;
        Ok(())
    }
    pub fn request_provision(
        &mut self,
        actor: &Principal,
        request: &ProvisionRequest,
        now: u64,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        identity::require_admin(&tx, actor)?;
        enqueue(&tx, request, now)?;
        tx.commit()?;
        Ok(())
    }
    pub fn pending_provision(&self, runner: &str) -> Result<Vec<ProvisionRequest>> {
        let mut statement=self.db.prepare("SELECT p.environment_id,p.template_id,p.generation FROM shared_provisioning p JOIN shared_environments e ON e.id=p.environment_id WHERE p.runner_id=?1 AND e.runner_id=?1 AND p.state='pending' ORDER BY p.created_at_ms,p.environment_id LIMIT 128")?;
        Ok(statement
            .query_map([runner], |r| {
                Ok(ProvisionRequest {
                    environment_id: r.get(0)?,
                    template_id: r.get(1)?,
                    generation: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn provision_receipt(
        &mut self,
        runner: &str,
        receipt: &ProvisionReceipt,
        now: u64,
    ) -> Result<()> {
        if receipt
            .error
            .as_ref()
            .is_some_and(|error| error.len() > 4096)
            || receipt.success && receipt.error.is_some()
        {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String,String,Option<String>)>=tx.query_row("SELECT p.template_id,p.state,p.error FROM shared_provisioning p JOIN shared_environments e ON e.id=p.environment_id WHERE p.environment_id=?1 AND p.runner_id=?2 AND e.runner_id=?2 AND p.generation=?3",params![receipt.environment_id,runner,receipt.generation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((template, state, error)) = previous else {
            return Err(WorkError::NotFound);
        };
        let next = if receipt.success { "ready" } else { "failed" };
        if template != receipt.template_id
            || state != "pending" && (state != next || error != receipt.error)
        {
            return Err(WorkError::Conflict);
        }
        tx.execute("UPDATE shared_provisioning SET state=?2,error=?3,updated_at_ms=?4 WHERE environment_id=?1",params![receipt.environment_id,next,receipt.error,now])?;
        if receipt.success {
            tx.execute("UPDATE shared_environments SET enabled=1 WHERE id=?1 AND EXISTS(SELECT 1 FROM shared_project_devices d JOIN shared_projects p ON p.id=d.project_id WHERE d.environment_id=?1 AND d.can_execute=1 AND p.archived=0)",[&receipt.environment_id])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn provision_catalog(&self, actor: &Principal) -> Result<Value> {
        identity::require_admin(&self.db, actor)?;
        let mut templates=self.db.prepare("SELECT runner_id,template_id,label FROM shared_runner_templates ORDER BY runner_id,template_id LIMIT 1000")?;
        let templates=templates.query_map([],|r|Ok(serde_json::json!({"runner_id":r.get::<_,String>(0)?,"id":r.get::<_,String>(1)?,"label":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut receipts=self.db.prepare("SELECT environment_id,runner_id,template_id,state,error,generation FROM shared_provisioning ORDER BY updated_at_ms DESC,environment_id LIMIT 1000")?;
        let receipts=receipts.query_map([],|r|Ok(serde_json::json!({"environment_id":r.get::<_,String>(0)?,"runner_id":r.get::<_,String>(1)?,"template_id":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?,"generation":r.get::<_,u64>(5)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(serde_json::json!({"templates":templates,"requests":receipts}))
    }
}

pub(super) fn enqueue(db: &Connection, request: &ProvisionRequest, now: u64) -> Result<()> {
    store::id(&request.environment_id)?;
    store::id(&request.template_id)?;
    store::timestamp(now)?;
    let runner:String=db.query_row("SELECT e.runner_id FROM shared_environments e JOIN shared_runner_templates t ON t.runner_id=e.runner_id AND t.template_id=?2 WHERE e.id=?1",params![request.environment_id,request.template_id],|r|r.get(0)).optional()?.ok_or(WorkError::NotFound)?;
    let previous: Option<(String, u64, String)> = db
        .query_row(
            "SELECT template_id,generation,state FROM shared_provisioning WHERE environment_id=?1",
            [&request.environment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((previous, generation, state)) = previous {
        if previous != request.template_id {
            return Err(WorkError::Conflict);
        }
        if generation == request.generation.saturating_add(1) {
            return Ok(());
        }
        if generation != request.generation || state != "failed" || generation >= 1000 {
            return Err(WorkError::Conflict);
        }
        db.execute("UPDATE shared_provisioning SET generation=generation+1,state='pending',error=NULL,updated_at_ms=?2 WHERE environment_id=?1",params![request.environment_id,now])?;
        return Ok(());
    }
    if request.generation != 0 {
        return Err(WorkError::Conflict);
    }
    if db.query_row("SELECT EXISTS(SELECT 1 FROM shared_attempts a JOIN shared_jobs j ON j.id=a.job_id WHERE j.environment_id=?1 AND a.state IN ('assigned','running','stop_requested'))",[&request.environment_id],|r|r.get::<_,bool>(0))? {return Err(WorkError::Conflict)}
    db.execute("INSERT INTO shared_provisioning(environment_id,runner_id,template_id,state,error,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'pending',NULL,?4,?4)",params![request.environment_id,runner,request.template_id,now])?;
    Ok(())
}
