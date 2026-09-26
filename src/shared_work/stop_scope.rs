//! Durable stop fences for a local user turn and a Hub project conversation.
use super::*;
use crate::identity::{self, Principal, ProjectRole};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

pub(super) fn validate_submission(
    db: &Connection,
    actor: &Principal,
    device: Option<&str>,
    request: &SubmitJob,
) -> Result<()> {
    let Some(origin) = request.origin_session_ref.as_deref() else {
        return if request.origin_turn_ref.is_none() && request.origin_turn_epoch.is_none() {
            Ok(())
        } else {
            Err(WorkError::Invalid)
        };
    };
    let Some(device) = device else {
        // Internal pre-V19 jobs had no attributable controller PC. They remain
        // valid legacy records, but can never claim a turn epoch.
        return if request.origin_turn_ref.is_none() && request.origin_turn_epoch.is_none() {
            Ok(())
        } else {
            Err(WorkError::Forbidden)
        };
    };
    if let (Some(turn), Some(epoch)) = (
        request.origin_turn_ref.as_deref(),
        request.origin_turn_epoch,
    ) {
        let admitted: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_origin_turns t
             JOIN shared_origin_scopes s USING(origin_device_id,origin_owner_id,origin_session_ref)
             WHERE t.origin_device_id=?1 AND t.origin_owner_id=?2 AND t.origin_session_ref=?3
               AND t.origin_turn_ref=?4 AND t.epoch=?5 AND t.stopped_at_ms IS NULL
               AND t.epoch>s.stop_floor)",
            params![device, actor.user_id, origin, turn, epoch],
            |row| row.get(0),
        )?;
        return if admitted {
            Ok(())
        } else {
            Err(WorkError::Conflict)
        };
    }
    // Old clients may finish their pre-stop work. Once any stop was requested
    // in this private scope they cannot submit an unidentifiable late turn.
    let fenced: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_origin_scopes s WHERE s.origin_device_id=?1
         AND s.origin_owner_id=?2 AND s.origin_session_ref=?3
         AND (s.stop_floor>0 OR s.stop_through_revision IS NOT NULL OR EXISTS(SELECT 1 FROM shared_origin_turns t
             WHERE t.origin_device_id=s.origin_device_id AND t.origin_owner_id=s.origin_owner_id
               AND t.origin_session_ref=s.origin_session_ref AND t.stopped_at_ms IS NOT NULL)))",
        params![device, actor.user_id, origin],
        |row| row.get(0),
    )?;
    if fenced {
        Err(WorkError::Conflict)
    } else {
        Ok(())
    }
}

pub(super) fn project_conversation_epoch(
    db: &Connection,
    project: &str,
    conversation: &str,
) -> Result<u64> {
    Ok(db.query_row(
        "SELECT current_epoch FROM shared_conversation_stop_fences WHERE project_id=?1 AND conversation_id=?2",
        params![project, conversation], |row| row.get(0),
    ).optional()?.unwrap_or(0))
}

pub(super) fn validate_continuation(db: &Connection, previous: &Job) -> Result<()> {
    if let Some(origin) = previous.origin_session_ref.as_deref() {
        let (device, revision): (Option<String>, Option<u64>) = db.query_row(
            "SELECT origin_device_id,origin_turn_revision FROM shared_jobs WHERE id=?1",
            [&previous.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if let Some(device) = device {
            let fenced: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM shared_origin_scopes s JOIN shared_jobs anchor
                  ON anchor.id=?1 AND anchor.id=anchor.root_id AND anchor.parent_id IS NULL
                 WHERE s.origin_device_id=?2 AND s.origin_owner_id=anchor.requestor_id
                   AND s.origin_session_ref=?3 AND s.stop_through_revision IS NOT NULL
                   AND COALESCE(?4,0)<=s.stop_through_revision)",
                params![previous.conversation_id, device, origin, revision],
                |r| r.get(0),
            )?;
            if fenced {
                return Err(WorkError::Conflict);
            }
        }
    }
    Ok(())
}

fn receipt_hash(
    operation: &str,
    device: &str,
    owner: &str,
    origin: &str,
    turn: Option<&str>,
    revision: u64,
) -> String {
    format!("{:x}", Sha256::digest(serde_json::json!({
        "operation":operation,"device":device,"owner":owner,"origin":origin,"turn":turn,"revision":revision
    }).to_string().as_bytes()))
}

fn previous_receipt(
    db: &Connection,
    actor: &Principal,
    request_id: &str,
    hash: &str,
) -> Result<bool> {
    let prior: Option<String> = db.query_row(
        "SELECT payload_hash FROM shared_management_receipts WHERE actor_id=?1 AND request_id=?2",
        params![actor.user_id, request_id], |row| row.get(0),
    ).optional()?;
    match prior {
        None => Ok(false),
        Some(prior) if prior == hash => Ok(true),
        Some(_) => Err(WorkError::Conflict),
    }
}

fn record_receipt(
    db: &Connection,
    actor: &Principal,
    request_id: &str,
    hash: &str,
    now: u64,
) -> Result<()> {
    super::management::capacity(db, "shared_management_receipts", 100_000)?;
    db.execute(
        "INSERT INTO shared_management_receipts(request_id,actor_id,payload_hash,created_at_ms) VALUES(?1,?2,?3,?4)",
        params![request_id, actor.user_id, hash, now],
    )?;
    Ok(())
}

fn scope_projects(
    db: &Connection,
    actor: &Principal,
    device: &str,
    origin: &str,
    turn: Option<(&str, u64)>,
    through_revision: u64,
) -> Result<Vec<String>> {
    let mut statement = db.prepare(
        "SELECT DISTINCT j.project_id FROM shared_jobs j JOIN shared_jobs anchor
           ON anchor.id=j.conversation_id AND anchor.id=anchor.root_id AND anchor.parent_id IS NULL
          AND anchor.conversation_id=anchor.id AND anchor.project_id=j.project_id
          AND anchor.origin_session_ref=j.origin_session_ref AND anchor.origin_device_id=j.origin_device_id
         WHERE j.id=j.root_id AND j.parent_id IS NULL AND j.origin_device_id=?1
           AND j.origin_session_ref=?2 AND anchor.requestor_id=?3
           AND ((?4 IS NULL AND COALESCE(j.origin_turn_revision,0)<=?6)
                OR (j.origin_turn_ref=?4 AND j.origin_turn_epoch=?5))
           AND (j.state NOT IN ('succeeded','failed','cancelled') OR EXISTS (
                SELECT 1 FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id
                JOIN shared_jobs sj ON sj.id=a.job_id WHERE sj.root_id=j.id AND s.state IN ('active','uncertain')))
         ORDER BY j.project_id"
    )?;
    let projects = statement
        .query_map(
            params![
                device,
                origin,
                actor.user_id,
                turn.map(|v| v.0),
                turn.map(|v| v.1),
                through_revision
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for project in &projects {
        identity::require_project_role(db, &actor.user_id, project, ProjectRole::Contributor)?;
        super::project_setup::require_project_controller(db, project, device)?;
    }
    Ok(projects)
}

fn scope_roots(
    db: &Connection,
    actor: &Principal,
    device: &str,
    origin: &str,
    turn: Option<(&str, u64)>,
    through_revision: u64,
) -> Result<Vec<String>> {
    let mut statement = db.prepare(
        "SELECT j.id FROM shared_jobs j JOIN shared_jobs anchor
           ON anchor.id=j.conversation_id AND anchor.id=anchor.root_id AND anchor.parent_id IS NULL
          AND anchor.conversation_id=anchor.id AND anchor.project_id=j.project_id
          AND anchor.origin_session_ref=j.origin_session_ref AND anchor.origin_device_id=j.origin_device_id
         WHERE j.id=j.root_id AND j.parent_id IS NULL AND j.origin_device_id=?1
           AND j.origin_session_ref=?2 AND anchor.requestor_id=?3
           AND ((?4 IS NULL AND COALESCE(j.origin_turn_revision,0)<=?6)
                OR (j.origin_turn_ref=?4 AND j.origin_turn_epoch=?5))
           AND j.state NOT IN ('succeeded','failed','cancelled')
         ORDER BY j.id"
    )?;
    Ok(statement
        .query_map(
            params![
                device,
                origin,
                actor.user_id,
                turn.map(|v| v.0),
                turn.map(|v| v.1),
                through_revision
            ],
            |row| row.get(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn stop_scope_work(
    db: &Connection,
    actor: &Principal,
    device: &str,
    origin: &str,
    turn: Option<(&str, u64)>,
    through_revision: u64,
    now: u64,
    stop_services: bool,
) -> Result<()> {
    for root in scope_roots(db, actor, device, origin, turn, through_revision)? {
        store::cancel_root_in_transaction(db, &root, now)?;
    }
    if stop_services {
        db.execute(
            "UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?7),updated_at_ms=?7
             WHERE state IN ('active','uncertain') AND service_id IN (
               SELECT s.service_id FROM shared_service_leases s
               JOIN shared_attempts a ON a.id=s.attempt_id JOIN shared_jobs j ON j.id=a.job_id
               JOIN shared_jobs anchor ON anchor.id=j.conversation_id AND anchor.id=anchor.root_id
                 AND anchor.parent_id IS NULL AND anchor.conversation_id=anchor.id
                 AND anchor.project_id=j.project_id AND anchor.origin_session_ref=j.origin_session_ref
                 AND anchor.origin_device_id=j.origin_device_id
               WHERE j.origin_device_id=?1 AND j.origin_session_ref=?2 AND anchor.requestor_id=?3
                 AND ((?4 IS NULL AND COALESCE(j.origin_turn_revision,0)<=?6)
                      OR (j.origin_turn_ref=?4 AND j.origin_turn_epoch=?5)))",
            params![device,origin,actor.user_id,turn.map(|v|v.0),turn.map(|v|v.1),through_revision,now],
        )?;
    }
    Ok(())
}

impl SharedWork {
    pub fn stop_project_conversation(
        &mut self,
        actor: &Principal,
        project: &str,
        conversation: &str,
        request_id: &str,
        now: u64,
    ) -> Result<Value> {
        for value in [project, conversation, request_id] {
            store::id(value)?;
        }
        store::timestamp(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let role =
            identity::require_project_role(&tx, &actor.user_id, project, ProjectRole::Contributor)?;
        let anchor:bool=tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_jobs WHERE id=?1 AND project_id=?2 AND id=root_id AND parent_id IS NULL AND conversation_id=id)",
            params![conversation,project],|r|r.get(0),
        )?;
        if !anchor {
            return Err(WorkError::NotFound);
        }
        let hash = receipt_hash(
            "stop_project_conversation",
            project,
            &actor.user_id,
            conversation,
            None,
            0,
        );
        if previous_receipt(&tx, actor, request_id, &hash)? {
            let epoch: u64 = tx.query_row(
                "SELECT conversation_epoch FROM shared_project_stop_receipts WHERE actor_id=?1 AND request_id=?2 AND project_id=?3 AND conversation_id=?4",
                params![actor.user_id,request_id,project,conversation],|row|row.get(0),
            ).optional()?.ok_or(WorkError::Conflict)?;
            return Ok(
                serde_json::json!({"accepted":true,"project_id":project,"conversation_id":conversation,"request_id":request_id,"conversation_epoch":epoch}),
            );
        }
        if role != ProjectRole::Manager {
            let foreign:bool=tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM shared_jobs j WHERE j.project_id=?1 AND j.conversation_id=?2
                 AND j.id=j.root_id AND j.parent_id IS NULL AND j.assignee_id<>?3
                 AND (j.state NOT IN ('succeeded','failed','cancelled') OR EXISTS (
                     SELECT 1 FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id
                     JOIN shared_jobs sj ON sj.id=a.job_id WHERE sj.root_id=j.id AND s.state IN ('active','uncertain'))))",
                params![project,conversation,actor.user_id],|r|r.get(0),
            )?;
            if foreign {
                return Err(WorkError::Forbidden);
            }
        }
        let mut statement=tx.prepare("SELECT id FROM shared_jobs WHERE project_id=?1 AND conversation_id=?2 AND id=root_id AND parent_id IS NULL AND state NOT IN ('succeeded','failed','cancelled') ORDER BY id")?;
        let roots = statement
            .query_map(params![project, conversation], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        for root in roots {
            store::cancel_root_in_transaction(&tx, &root, now)?;
        }
        tx.execute(
            "UPDATE shared_service_leases SET stop_requested_at_ms=COALESCE(stop_requested_at_ms,?3),updated_at_ms=?3
             WHERE state IN ('active','uncertain') AND service_id IN (
               SELECT s.service_id FROM shared_service_leases s JOIN shared_attempts a ON a.id=s.attempt_id
               JOIN shared_jobs j ON j.id=a.job_id WHERE j.project_id=?1 AND j.conversation_id=?2)",
            params![project,conversation,now],
        )?;
        let prior_epoch = project_conversation_epoch(&tx, project, conversation)?;
        if prior_epoch >= i64::MAX as u64 {
            return Err(WorkError::Capacity);
        }
        let epoch = prior_epoch + 1;
        tx.execute(
            "INSERT INTO shared_conversation_stop_fences(project_id,conversation_id,stopped_at_ms,current_epoch)
             VALUES(?1,?2,?3,?4) ON CONFLICT(project_id,conversation_id)
             DO UPDATE SET stopped_at_ms=excluded.stopped_at_ms,current_epoch=excluded.current_epoch",
            params![project,conversation,now,epoch],
        )?;
        record_receipt(&tx, actor, request_id, &hash, now)?;
        tx.execute(
            "INSERT INTO shared_project_stop_receipts(actor_id,request_id,project_id,conversation_id,conversation_epoch) VALUES(?1,?2,?3,?4,?5)",
            params![actor.user_id,request_id,project,conversation,epoch],
        )?;
        tx.commit()?;
        Ok(
            serde_json::json!({"accepted":true,"project_id":project,"conversation_id":conversation,"request_id":request_id,"conversation_epoch":epoch}),
        )
    }

    pub fn admit_origin_turn(
        &mut self,
        actor: &Principal,
        device: &str,
        origin: &str,
        turn: &str,
        request_id: &str,
        create: bool,
        turn_revision: u64,
        now: u64,
    ) -> Result<Value> {
        for value in [device, origin, turn, request_id] {
            store::id(value)?;
        }
        store::timestamp(now)?;
        if turn_revision == 0 || turn_revision > i64::MAX as u64 {
            return Err(WorkError::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing:Option<(u64,Option<u64>,u64)>=tx.query_row(
            "SELECT epoch,stopped_at_ms,turn_revision FROM shared_origin_turns WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND origin_turn_ref=?4",
            params![device,actor.user_id,origin,turn],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        let epoch = if let Some((epoch, stopped, prior_revision)) = existing {
            if prior_revision != turn_revision {
                return Err(WorkError::Conflict);
            }
            let floor:u64=tx.query_row("SELECT stop_floor FROM shared_origin_scopes WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3",params![device,actor.user_id,origin],|r|r.get(0))?;
            if stopped.is_some() || epoch <= floor {
                return Err(WorkError::Conflict);
            }
            epoch
        } else {
            if !create {
                return Err(WorkError::Conflict);
            }
            tx.execute("INSERT OR IGNORE INTO shared_origin_scopes(origin_device_id,origin_owner_id,origin_session_ref,current_epoch,stop_floor) VALUES(?1,?2,?3,0,0)",params![device,actor.user_id,origin])?;
            let (stopped_through,last_revision):(Option<u64>,u64)=tx.query_row(
                "SELECT stop_through_revision,COALESCE((SELECT MAX(turn_revision) FROM shared_origin_turns t WHERE t.origin_device_id=s.origin_device_id AND t.origin_owner_id=s.origin_owner_id AND t.origin_session_ref=s.origin_session_ref),0) FROM shared_origin_scopes s WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3",
                params![device,actor.user_id,origin],|r|Ok((r.get(0)?,r.get(1)?)),
            )?;
            if stopped_through.is_some_and(|floor| turn_revision <= floor)
                || turn_revision <= last_revision
            {
                return Err(WorkError::Conflict);
            }
            tx.execute("UPDATE shared_origin_scopes SET current_epoch=current_epoch+1 WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND current_epoch<9223372036854775807",params![device,actor.user_id,origin])?;
            let epoch:u64=tx.query_row("SELECT current_epoch FROM shared_origin_scopes WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3",params![device,actor.user_id,origin],|r|r.get(0))?;
            if epoch == 0 || epoch == i64::MAX as u64 {
                return Err(WorkError::Capacity);
            }
            tx.execute("INSERT INTO shared_origin_turns(origin_device_id,origin_owner_id,origin_session_ref,origin_turn_ref,turn_revision,epoch) VALUES(?1,?2,?3,?4,?5,?6)",params![device,actor.user_id,origin,turn,turn_revision,epoch])?;
            epoch
        };
        tx.commit()?;
        Ok(serde_json::json!({"origin_session_ref":origin,"origin_turn_ref":turn,"epoch":epoch}))
    }

    pub fn stop_origin_turn(
        &mut self,
        actor: &Principal,
        device: &str,
        origin: &str,
        turn: &str,
        turn_revision: u64,
        request_id: &str,
        now: u64,
    ) -> Result<Value> {
        self.stop_origin_scope(
            actor,
            device,
            origin,
            Some((turn, turn_revision)),
            turn_revision,
            request_id,
            now,
        )
    }

    pub fn stop_origin(
        &mut self,
        actor: &Principal,
        device: &str,
        origin: &str,
        through_revision: u64,
        request_id: &str,
        now: u64,
    ) -> Result<Value> {
        self.stop_origin_scope(
            actor,
            device,
            origin,
            None,
            through_revision,
            request_id,
            now,
        )
    }

    fn stop_origin_scope(
        &mut self,
        actor: &Principal,
        device: &str,
        origin: &str,
        turn: Option<(&str, u64)>,
        through_revision: u64,
        request_id: &str,
        now: u64,
    ) -> Result<Value> {
        for value in [device, origin, request_id] {
            store::id(value)?;
        }
        if let Some((turn, revision)) = turn {
            store::id(turn)?;
            if revision == 0 || revision > i64::MAX as u64 {
                return Err(WorkError::Invalid);
            }
        }
        if through_revision > i64::MAX as u64 {
            return Err(WorkError::Invalid);
        }
        store::timestamp(now)?;
        let hash = receipt_hash(
            if turn.is_some() {
                "stop_origin_turn"
            } else {
                "stop_origin"
            },
            device,
            &actor.user_id,
            origin,
            turn.map(|v| v.0),
            through_revision,
        );
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let response = serde_json::json!({"accepted":true,"origin_session_ref":origin,"origin_turn_ref":turn.map(|v|v.0),"request_id":request_id});
        if previous_receipt(&tx, actor, request_id, &hash)? {
            return Ok(response);
        }
        tx.execute("INSERT OR IGNORE INTO shared_origin_scopes(origin_device_id,origin_owner_id,origin_session_ref,current_epoch,stop_floor) VALUES(?1,?2,?3,0,0)",params![device,actor.user_id,origin])?;
        let scoped_turn = if let Some((turn, revision)) = turn {
            let existing:Option<(u64,u64)>=tx.query_row(
                "SELECT epoch,turn_revision FROM shared_origin_turns WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND origin_turn_ref=?4",
                params![device,actor.user_id,origin,turn],|r|Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            let epoch = if let Some((epoch, old_revision)) = existing {
                if old_revision != revision {
                    return Err(WorkError::Conflict);
                }
                epoch
            } else {
                tx.execute("UPDATE shared_origin_scopes SET current_epoch=current_epoch+1 WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND current_epoch<9223372036854775807",params![device,actor.user_id,origin])?;
                let epoch:u64=tx.query_row("SELECT current_epoch FROM shared_origin_scopes WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3",params![device,actor.user_id,origin],|r|r.get(0))?;
                if epoch == 0 || epoch == i64::MAX as u64 {
                    return Err(WorkError::Capacity);
                }
                tx.execute("INSERT INTO shared_origin_turns(origin_device_id,origin_owner_id,origin_session_ref,origin_turn_ref,turn_revision,epoch,stopped_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![device,actor.user_id,origin,turn,revision,epoch,now])?;
                epoch
            };
            tx.execute("UPDATE shared_origin_turns SET stopped_at_ms=COALESCE(stopped_at_ms,?5) WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND origin_turn_ref=?4",params![device,actor.user_id,origin,turn,now])?;
            Some((turn, epoch))
        } else {
            tx.execute("UPDATE shared_origin_scopes SET stop_through_revision=MAX(COALESCE(stop_through_revision,?4),?4),stop_floor=MAX(stop_floor,COALESCE((SELECT MAX(epoch) FROM shared_origin_turns t WHERE t.origin_device_id=shared_origin_scopes.origin_device_id AND t.origin_owner_id=shared_origin_scopes.origin_owner_id AND t.origin_session_ref=shared_origin_scopes.origin_session_ref AND t.turn_revision<=?4),0)) WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3",params![device,actor.user_id,origin,through_revision])?;
            tx.execute("UPDATE shared_origin_turns SET stopped_at_ms=COALESCE(stopped_at_ms,?4) WHERE origin_device_id=?1 AND origin_owner_id=?2 AND origin_session_ref=?3 AND turn_revision<=?5",params![device,actor.user_id,origin,now,through_revision])?;
            None
        };
        scope_projects(&tx, actor, device, origin, scoped_turn, through_revision)?;
        stop_scope_work(
            &tx,
            actor,
            device,
            origin,
            scoped_turn,
            through_revision,
            now,
            true,
        )?;
        record_receipt(&tx, actor, request_id, &hash, now)?;
        tx.commit()?;
        Ok(response)
    }
}
