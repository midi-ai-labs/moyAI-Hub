//! FIFO admission for requests that can currently use a selected deployment.
//! Tickets order metadata attempts; the Gateway still owns live relay capacity.
use super::*;

const MAX_QUEUED_REQUESTS: usize = MAX_CLIENTS * MAX_CONTEXTS_PER_CLIENT;

#[derive(Clone)]
pub(super) struct RequestTicket {
    pub client_id: String,
    pub context_id: String,
    pub turn_id: String,
    request_id: String,
    last_polled_ms: u64,
}
impl RequestTicket {
    fn matches(&self, client: &str, context: &str, turn: &str, request: &str) -> bool {
        self.client_id == client
            && self.context_id == context
            && self.turn_id == turn
            && self.request_id == request
    }
}

impl HubCore {
    /// Network prepare's single request-admission entrypoint. Retries retain
    /// their ticket through a companion capacity decline; successful companion
    /// admission removes it with `request_admitted`.
    pub fn prepare_request(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
        request_id: &str,
        purpose: RequestPurpose,
        now_ms: u64,
    ) -> Result<PermitDecision, HubError> {
        check_token(request_id)?;
        self.prune_request_queue(now_ms);
        let replay = self
            .checked_client(id, now_ms)?
            .contexts
            .get(context_id)
            .and_then(|context| context.active_turn.as_ref())
            .is_some_and(|turn| turn.id == turn_id && turn.requests.contains_key(request_id));
        let queued = self
            .request_queue
            .iter()
            .any(|ticket| ticket.matches(id, context_id, turn_id, request_id));
        if !queued && !replay && self.request_queue.len() >= MAX_QUEUED_REQUESTS {
            return Err(HubError::RegistryFull);
        }
        let route = self.begin_request(id, context_id, turn_id, purpose, now_ms)?;
        if let Some(ticket) = self
            .request_queue
            .iter_mut()
            .find(|ticket| ticket.matches(id, context_id, turn_id, request_id))
        {
            ticket.last_polled_ms = now_ms;
        }
        // A previously issued request is an acknowledgement, not a new queue
        // entrant or attempt; the existing exact permit/expiry check remains.
        if let RouteDecision::Assigned { lease } = &route {
            if replay {
                return self.acquire_permit(id, context_id, &lease.id, request_id, now_ms);
            }
        }
        if !self
            .request_queue
            .iter()
            .any(|ticket| ticket.matches(id, context_id, turn_id, request_id))
        {
            self.request_queue.push_back(RequestTicket {
                client_id: id.into(),
                context_id: context_id.into(),
                turn_id: turn_id.into(),
                request_id: request_id.into(),
                last_polled_ms: now_ms,
            });
        }
        let lease = match route {
            RouteDecision::Assigned { lease } => lease,
            RouteDecision::Waiting {
                reason,
                retry_after_ms,
            } => {
                return Ok(PermitDecision::Waiting {
                    reason,
                    retry_after_ms,
                });
            }
        };
        if let Some(reason) = self.queue_wait_reason(
            id,
            context_id,
            turn_id,
            request_id,
            &lease.deployment_id,
            now_ms,
        ) {
            return Ok(PermitDecision::Waiting {
                reason,
                retry_after_ms: RETRY_AFTER_MS,
            });
        }
        self.acquire_permit(id, context_id, &lease.id, request_id, now_ms)
    }

    pub(crate) fn request_admitted(&mut self, permit: &ExecutionPermit) {
        self.request_queue.retain(|ticket| {
            !ticket.matches(
                &permit.client_id,
                &permit.context_id,
                &permit.turn_id,
                &permit.request_id,
            )
        });
    }

    fn prune_request_queue(&mut self, now_ms: u64) {
        let mut tickets = std::mem::take(&mut self.request_queue);
        tickets.retain(|ticket| {
            // A live Desktop heartbeat does not keep an abandoned request in
            // front of others indefinitely. Normal retry is once per second.
            if now_ms.saturating_sub(ticket.last_polled_ms)
                >= self.store.catalog.policy.heartbeat_stale_ms
            {
                return false;
            }
            let Ok(client) = self.checked_client(&ticket.client_id, now_ms) else {
                return false;
            };
            let Some(context) = client.contexts.get(&ticket.context_id) else {
                return false;
            };
            if self.validate_selection(&context.selection).is_err() {
                return false;
            }
            context
                .active_turn
                .as_ref()
                .is_some_and(|turn| turn.id == ticket.turn_id && turn.lease.expires_at_ms > now_ms)
                || context
                    .waiting_turn
                    .as_ref()
                    .is_some_and(|(turn, _)| turn == &ticket.turn_id)
        });
        self.request_queue = tickets;
    }

    fn queued_request_pool(&self, ticket: &RequestTicket, now_ms: u64) -> Option<String> {
        let context = self
            .clients
            .get(&ticket.client_id)?
            .contexts
            .get(&ticket.context_id)?;
        // A pending companion acknowledgement already occupies a core permit.
        // It must not count as another virtual slot ahead of unrelated work.
        if context
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.requests.contains_key(&ticket.request_id))
        {
            return None;
        }
        let pinned = context
            .active_turn
            .as_ref()
            .map(|turn| &turn.lease)
            .or_else(|| {
                context.lease.as_ref().filter(|lease| {
                    lease.catalog_revision == self.store.revision
                        && lease.expires_at_ms > now_ms
                        && lease.turns_remaining > 0
                })
            });
        if let Some(lease) = pinned {
            return self
                .store
                .catalog
                .deployments
                .iter()
                .find(|deployment| deployment.id == lease.deployment_id)
                .filter(|deployment| {
                    matches!(self.unavailable_reason(deployment, now_ms), Ok(None))
                })
                .map(|deployment| deployment.capacity_pool_id.clone());
        }
        self.select_deployment(&context.selection, now_ms)
            .ok()
            .map(|deployment| deployment.capacity_pool_id)
    }

    fn queue_wait_reason(
        &self,
        id: &str,
        context: &str,
        turn: &str,
        request: &str,
        deployment_id: &str,
        now_ms: u64,
    ) -> Option<WaitReason> {
        let target = self
            .store
            .catalog
            .deployments
            .iter()
            .find(|deployment| deployment.id == deployment_id)?;
        let mut global = self.store.catalog.policy.global_max_concurrent;
        let mut available: BTreeMap<_, _> = self
            .store
            .catalog
            .capacity_pools
            .iter()
            .map(|pool| (pool.id.clone(), pool.max_concurrent))
            .collect();
        for permit in self
            .permits
            .values()
            .filter(|permit| permit.permit.expires_at_ms > now_ms)
        {
            global = global.saturating_sub(1);
            if let Some(slots) = available.get_mut(&permit.capacity_pool_id) {
                *slots = slots.saturating_sub(1);
            }
        }
        // Account for earlier runnable tickets only while they fit. This does
        // not allocate permits: it avoids head-of-line blocking across pools
        // while leaving room for earlier callers when they next poll.
        for ticket in self
            .request_queue
            .iter()
            .take_while(|ticket| !ticket.matches(id, context, turn, request))
        {
            let Some(pool) = self.queued_request_pool(ticket, now_ms) else {
                continue;
            };
            if let Some(slots) = available.get_mut(&pool) {
                if *slots > 0 && global > 0 {
                    *slots -= 1;
                    global -= 1;
                }
            }
        }
        if available
            .get(&target.capacity_pool_id)
            .copied()
            .unwrap_or(0)
            == 0
        {
            return Some(WaitReason::Busy);
        }
        (global == 0).then_some(WaitReason::GlobalCapacity)
    }
}

#[cfg(test)]
mod tests;
