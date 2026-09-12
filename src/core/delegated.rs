//! Delegated execution reuses the existing route/permit/queue owner with an
//! immutable copy of a reviewed Main or Side selection and an independent scope.
use super::*;

const MAX_DELEGATED_CONTEXTS: usize = MAX_CONTEXTS_PER_CLIENT - 2;
const MAX_DELEGATIONS_PER_CLIENT: usize = 4096;

#[derive(Clone)]
pub(super) struct DelegatedContext {
    pub parent: String,
    pub last_seen_ms: u64,
}
impl DelegatedContext {
    pub(super) fn expired(&self, now_ms: u64, timeout_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) >= timeout_ms
    }
}

impl HubCore {
    pub fn delegated_context_id(parent: &str, execution: &str) -> Result<String, HubError> {
        if !matches!(parent, "main" | "side_chat")
            || ulid::Ulid::from_string(execution)
                .ok()
                .is_none_or(|id| id.to_string() != execution)
        {
            return Err(HubError::InvalidRequest {
                message: "invalid delegated execution identity".into(),
            });
        }
        Ok(format!("delegated-{parent}-{execution}"))
    }

    pub fn ensure_delegated_context(
        &mut self,
        id: &str,
        parent: &str,
        execution: &str,
        selection: Selection,
        now_ms: u64,
    ) -> Result<String, HubError> {
        let context_id = Self::delegated_context_id(parent, execution)?;
        self.expire_delegated_contexts(id, now_ms)?;
        self.validate_selection(&selection)?;
        let client = self.checked_client(id, now_ms)?;
        if client.closed_delegations.contains_key(execution) {
            return Err(HubError::TurnClosed);
        }
        if let Some(existing) = client.contexts.get(&context_id) {
            if existing
                .delegated
                .as_ref()
                .map(|scope| scope.parent.as_str())
                != Some(parent)
            {
                return Err(HubError::WrongOwner);
            }
            if existing.selection != selection {
                return Err(HubError::ReviewRequired {
                    current_revision: self.store.revision,
                });
            }
            self.clients
                .get_mut(id)
                .unwrap()
                .contexts
                .get_mut(&context_id)
                .unwrap()
                .delegated
                .as_mut()
                .unwrap()
                .last_seen_ms = now_ms;
            return Ok(context_id);
        }
        for other in ["main", "side_chat"] {
            if other != parent
                && client
                    .contexts
                    .contains_key(&Self::delegated_context_id(other, execution)?)
            {
                return Err(HubError::WrongOwner);
            }
        }
        let reviewed = client
            .contexts
            .get(parent)
            .ok_or(HubError::SelectionRequired)?;
        if reviewed.selection != selection {
            return Err(HubError::ReviewRequired {
                current_revision: self.store.revision,
            });
        }
        let active_count = client
            .contexts
            .values()
            .filter(|context| context.delegated.is_some())
            .count();
        if client.contexts.len() >= MAX_CONTEXTS_PER_CLIENT
            || active_count >= MAX_DELEGATED_CONTEXTS
            || active_count + client.closed_delegations.len() >= MAX_DELEGATIONS_PER_CLIENT
        {
            return Err(HubError::RegistryFull);
        }
        self.clients
            .get_mut(id)
            .ok_or(HubError::UnknownClient)?
            .contexts
            .insert(
                context_id.clone(),
                RouteContext {
                    delegated: Some(DelegatedContext {
                        parent: parent.into(),
                        last_seen_ms: now_ms,
                    }),
                    selection,
                    lease: None,
                    active_turn: None,
                    // Makes cancellation during metadata discovery an exact live-owner close.
                    waiting_turn: Some((execution.into(), RequestPurpose::Delegated)),
                    recent_closed_turns: VecDeque::new(),
                },
            );
        Ok(context_id)
    }

    pub fn close_delegated_context(
        &mut self,
        id: &str,
        parent: &str,
        execution: &str,
        canceled: bool,
    ) -> Result<String, HubError> {
        let context_id = Self::delegated_context_id(parent, execution)?;
        let client = self.clients.get(id).ok_or(HubError::UnknownClient)?;
        if let Some(closed_parent) = client.closed_delegations.get(execution) {
            return if closed_parent == parent {
                Ok(context_id)
            } else {
                Err(HubError::WrongOwner)
            };
        }
        if let Some(context) = client.contexts.get(&context_id) {
            if context
                .delegated
                .as_ref()
                .map(|scope| scope.parent.as_str())
                != Some(parent)
            {
                return Err(HubError::WrongOwner);
            }
            self.close_turn(id, &context_id, execution, canceled)?;
        } else {
            // A Stop can overtake the first prepare; keep its identity closed so
            // delayed admission cannot resurrect the child.
            if !client.contexts.contains_key(parent) {
                return Err(HubError::SelectionRequired);
            }
            if client
                .contexts
                .values()
                .filter(|context| context.delegated.is_some())
                .count()
                + client.closed_delegations.len()
                >= MAX_DELEGATIONS_PER_CLIENT
            {
                return Err(HubError::RegistryFull);
            }
            for other in ["main", "side_chat"] {
                if other != parent
                    && client
                        .contexts
                        .contains_key(&Self::delegated_context_id(other, execution)?)
                {
                    return Err(HubError::WrongOwner);
                }
            }
        }
        let client = self.clients.get_mut(id).ok_or(HubError::UnknownClient)?;
        client.contexts.remove(&context_id);
        client
            .closed_delegations
            .insert(execution.into(), parent.into());
        Ok(context_id)
    }

    pub(super) fn expire_delegated_contexts(
        &mut self,
        id: &str,
        now_ms: u64,
    ) -> Result<(), HubError> {
        let expired: Vec<_> = self
            .clients
            .get(id)
            .ok_or(HubError::UnknownClient)?
            .contexts
            .iter()
            .filter_map(|(context_id, context)| {
                let delegated = context.delegated.as_ref()?;
                if !delegated.expired(now_ms, self.store.catalog.policy.heartbeat_stale_ms) {
                    return None;
                }
                // Internal keys are produced only by delegated_context_id. Retiring
                // metadata never claims that an already running Gateway HTTP ended.
                let execution =
                    context_id.strip_prefix(&format!("delegated-{}-", delegated.parent))?;
                Some((delegated.parent.clone(), execution.to_owned()))
            })
            .collect();
        for (parent, execution) in expired {
            self.close_delegated_context(id, &parent, &execution, true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
