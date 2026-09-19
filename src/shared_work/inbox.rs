//! Notifications derive from canonical jobs/approvals/handovers; only read receipts are stored.
use super::*;
use crate::identity::{self, Principal};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

const EVENTS: &str = r#"
WITH events AS (
 SELECT printf('%020d',p.created_at_ms)||':approval:'||p.id id,j.id job_id,j.project_id,'approval' kind,j.title,p.created_at_ms,
 CASE WHEN p.decision IS NULL AND p.invalidated_at_ms IS NULL AND p.expires_at_ms>?2 AND a.state='running' AND a.uncertainty_reason IS NULL AND j.state='running' THEN 1 ELSE 0 END can_act,
 j.assignee_id recipient,j.requestor_id requestor,NULL third,p.id approval_id
 FROM shared_approvals p JOIN shared_attempts a ON a.id=p.attempt_id JOIN shared_jobs j ON j.id=a.job_id
 UNION ALL SELECT printf('%020d',j.updated_at_ms)||':job:'||j.id,j.id,j.project_id,'finished',j.title,j.updated_at_ms,0,j.assignee_id,j.requestor_id,NULL,NULL FROM shared_jobs j WHERE j.state IN ('succeeded','failed','cancelled')
 UNION ALL SELECT printf('%020d',h.requested_at_ms)||':handover:'||h.id,j.id,j.project_id,'handover',j.title,h.requested_at_ms,0,h.new_assignee_id,h.requested_by,h.old_assignee_id,NULL FROM shared_handovers h JOIN shared_jobs j ON j.id=h.root_id
), visible AS (
 SELECT e.*,r.read_at_ms,CASE WHEN e.can_act=1 AND (m.role='manager' OR (m.role='contributor' AND e.recipient=?1)) THEN 1 ELSE 0 END effective_can_act FROM events e JOIN identity_effective_project_memberships m ON m.project_id=e.project_id AND m.user_id=?1 LEFT JOIN shared_inbox_reads r ON r.user_id=?1 AND r.item_id=e.id
 WHERE (e.recipient=?1 OR e.requestor=?1 OR e.third=?1 OR (e.kind='approval' AND m.role='manager'))
 AND (?3 IS NULL OR e.project_id IN (SELECT value FROM json_each(?3)))
)
"#;
impl SharedWork {
    pub fn inbox(
        &self,
        actor: &Principal,
        before: Option<&str>,
        limit: u32,
        now: u64,
    ) -> Result<Value> {
        self.inbox_for_device(actor, before, limit, now, None)
    }
    pub fn inbox_for_device(
        &self,
        actor: &Principal,
        before: Option<&str>,
        limit: u32,
        now: u64,
        device: Option<&str>,
    ) -> Result<Value> {
        identity::active_principal(&self.db, &actor.user_id)?;
        let controlled = device
            .map(|id| self.controlled_projects(id))
            .transpose()?
            .map(|ids| serde_json::to_string(&ids))
            .transpose()?;
        if !(1..=100).contains(&limit) || before.is_some_and(|v| v.len() > 200) {
            return Err(WorkError::Invalid);
        }
        let unread: u64 = self.db.query_row(
            &format!("{EVENTS} SELECT COUNT(*) FROM visible WHERE read_at_ms IS NULL"),
            params![actor.user_id, now, controlled],
            |r| r.get(0),
        )?;
        let mut items=self.db.prepare(&format!("{EVENTS} SELECT id,job_id,project_id,kind,title,created_at_ms,read_at_ms,effective_can_act,approval_id FROM visible WHERE (?4 IS NULL OR id<?4) ORDER BY id DESC LIMIT ?5"))?.query_map(params![actor.user_id,now,controlled,before,limit+1],|r|Ok(json!({"id":r.get::<_,String>(0)?,"job_id":r.get::<_,String>(1)?,"project_id":r.get::<_,String>(2)?,"kind":r.get::<_,String>(3)?,"title":r.get::<_,String>(4)?,"created_at_ms":r.get::<_,u64>(5)?,"read_at_ms":r.get::<_,Option<u64>>(6)?,"can_act":r.get::<_,bool>(7)?,"approval_id":r.get::<_,Option<String>>(8)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next = if more {
            items.last().map(|v| v["id"].clone())
        } else {
            None
        };
        Ok(json!({"items":items,"next_before":next,"unread_count":unread}))
    }
    pub fn read_inbox(&mut self, actor: &Principal, item_id: &str, now: u64) -> Result<Value> {
        self.read_inbox_for_device(actor, item_id, now, None)
    }
    pub fn read_inbox_for_device(
        &mut self,
        actor: &Principal,
        item_id: &str,
        now: u64,
        device: Option<&str>,
    ) -> Result<Value> {
        identity::active_principal(&self.db, &actor.user_id)?;
        let controlled = device
            .map(|id| self.controlled_projects(id))
            .transpose()?
            .map(|ids| serde_json::to_string(&ids))
            .transpose()?;
        super::store::timestamp(now)?;
        if item_id.len() > 200 {
            return Err(WorkError::Invalid);
        }
        let found: Option<String> = self
            .db
            .query_row(
                &format!("{EVENTS} SELECT id FROM visible WHERE id=?4"),
                params![actor.user_id, now, controlled, item_id],
                |r| r.get(0),
            )
            .optional()?;
        if found.is_none() {
            return Err(WorkError::NotFound);
        }
        self.db.execute("INSERT INTO shared_inbox_reads VALUES(?1,?2,?3) ON CONFLICT(user_id,item_id) DO NOTHING",params![actor.user_id,item_id,now])?;
        Ok(json!({"read":true}))
    }
}
