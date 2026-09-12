use crate::catalog::{Catalog, CatalogStore, Deployment, DeploymentRow, valid_token};
use crate::protocol::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod delegated;
use delegated::DelegatedContext;
mod queue;
use queue::RequestTicket;

const MAX_CLIENTS: usize = 256;
const MAX_CONTEXTS_PER_CLIENT: usize = 16;
const MAX_REQUESTS_PER_TURN: usize = 4096;
const MAX_RECENT_TURNS: usize = 256;
const RETRY_AFTER_MS: u64 = 1000;

#[derive(Clone)]
struct Observation {
    availability: Availability,
    observed_at_ms: u64,
    model_present: bool,
}

#[derive(Clone)]
struct ActiveTurn {
    id: String,
    purpose: RequestPurpose,
    lease: RouteLease,
    requests: BTreeMap<String, String>,
}

#[derive(Clone)]
struct Client {
    id: String,
    label: String,
    observed_ip: String,
    last_seen_ms: u64,
    activity: ClientActivity,
    contexts: BTreeMap<String, RouteContext>,
    closed_delegations: BTreeMap<String, String>,
}

#[derive(Clone)]
struct RouteContext {
    delegated: Option<DelegatedContext>,
    selection: Selection,
    lease: Option<RouteLease>,
    active_turn: Option<ActiveTurn>,
    waiting_turn: Option<(String, RequestPurpose)>,
    recent_closed_turns: VecDeque<String>,
}

#[derive(Clone)]
struct PermitState {
    permit: ExecutionPermit,
    capacity_pool_id: String,
}

enum RouteBlock {
    Wait(WaitReason),
    Failure(HubError),
}

/// One in-memory owner. The embedding service serializes calls with a mutex.
/// Scheduling is advisory: expiry is not evidence that a provider stopped executing.
#[derive(Clone)]
pub struct HubCore {
    store: CatalogStore,
    observations: BTreeMap<String, Observation>,
    clients: BTreeMap<String, Client>,
    permits: BTreeMap<String, PermitState>,
    request_queue: VecDeque<RequestTicket>,
}

impl HubCore {
    pub fn new(store: CatalogStore) -> Result<Self, HubError> {
        store.validate()?;
        Ok(Self {
            store,
            observations: BTreeMap::new(),
            clients: BTreeMap::new(),
            permits: BTreeMap::new(),
            request_queue: VecDeque::new(),
        })
    }

    pub fn catalog_store(&self) -> &CatalogStore {
        &self.store
    }

    pub fn replace_catalog(&mut self, catalog: Catalog, now_ms: u64) -> Result<u64, HubError> {
        let unchanged_deployments: BTreeSet<String> = catalog
            .deployments
            .iter()
            .filter(|new| self.store.catalog.deployments.iter().any(|old| old == *new))
            .map(|deployment| deployment.id.clone())
            .collect();
        let previous_revision = self.store.revision;
        let revision = self.store.replace_catalog(catalog, now_ms)?;
        if revision != previous_revision {
            self.request_queue.clear();
        }
        self.observations
            .retain(|id, _| unchanged_deployments.contains(id));
        // Existing requests may finish. The revision gate rejects any new admission.
        Ok(revision)
    }

    pub fn observe_deployment(
        &mut self,
        id: &str,
        availability: Availability,
        now_ms: u64,
    ) -> Result<(), HubError> {
        let model_present = matches!(availability, Availability::Ready | Availability::JitPending);
        self.observe(id, availability, model_present, now_ms)
    }

    /// A successful provider listing establishes existence, never inferred load readiness.
    pub fn observe_present_deployment(
        &mut self,
        id: &str,
        availability: Availability,
        now_ms: u64,
    ) -> Result<(), HubError> {
        if !matches!(
            availability,
            Availability::Unknown | Availability::Ready | Availability::JitPending
        ) {
            return Err(HubError::InvalidRequest {
                message: "presence contradicts availability".into(),
            });
        }
        self.observe(id, availability, true, now_ms)
    }

    fn observe(
        &mut self,
        id: &str,
        availability: Availability,
        model_present: bool,
        now_ms: u64,
    ) -> Result<(), HubError> {
        if !self
            .store
            .catalog
            .deployments
            .iter()
            .any(|deployment| deployment.id == id)
        {
            return Err(HubError::UnknownDeployment);
        }
        self.observations.insert(
            id.to_owned(),
            Observation {
                availability,
                observed_at_ms: now_ms,
                model_present,
            },
        );
        Ok(())
    }

    pub fn routing_deployments(
        &self,
        id: &str,
        context_id: &str,
        now_ms: u64,
    ) -> Result<Vec<Deployment>, HubError> {
        let client = self.checked_client(id, now_ms)?;
        let context = client
            .contexts
            .get(context_id)
            .ok_or(HubError::SelectionRequired)?;
        self.validate_selection(&context.selection)?;
        Ok(self
            .store
            .catalog
            .deployments
            .iter()
            .filter(|deployment| {
                context
                    .selection
                    .allowed_model_ids
                    .contains(&deployment.logical_model_id)
            })
            .cloned()
            .collect())
    }

    pub fn needs_presence_refresh(&self, deployment_id: &str, now_ms: u64) -> bool {
        !self.model_present(deployment_id, now_ms)
    }

    /// The gateway calls this at claim time. A prepared token cannot bypass a later review,
    /// client revocation, turn cancellation or reservation expiry.
    pub fn validate_execution_permit(
        &self,
        permit_id: &str,
        now_ms: u64,
    ) -> Result<ExecutionPermit, HubError> {
        let state = self.permits.get(permit_id).ok_or(HubError::PermitExpired)?;
        let permit = &state.permit;
        if permit.expires_at_ms <= now_ms {
            return Err(HubError::PermitExpired);
        }
        let client = self.checked_client(&permit.client_id, now_ms)?;
        let context = client
            .contexts
            .get(&permit.context_id)
            .ok_or(HubError::SelectionRequired)?;
        if context.delegated.as_ref().is_some_and(|delegated| {
            delegated.expired(now_ms, self.store.catalog.policy.heartbeat_stale_ms)
        }) {
            return Err(HubError::TurnClosed);
        }
        self.validate_selection(&context.selection)?;
        let turn = context.active_turn.as_ref().ok_or(HubError::TurnClosed)?;
        if turn.id != permit.turn_id || turn.lease.id != permit.lease_id {
            return Err(HubError::WrongOwner);
        }
        Ok(permit.clone())
    }

    /// Only the authenticated companion reports transport settlement. Advisory expiration
    /// remains distinct from the companion's running/uncertain execution capacity.
    pub fn settle_execution_permit(&mut self, permit_id: &str) {
        self.permits.remove(permit_id);
    }

    /// Called only after the companion explicitly declined this reservation. Unlike
    /// settlement, this did not consume a request attempt and permits the same ID to wait.
    pub(crate) fn withdraw_declined_permit(&mut self, permit: &ExecutionPermit) {
        if let Some(turn) = self
            .clients
            .get_mut(&permit.client_id)
            .and_then(|client| client.contexts.get_mut(&permit.context_id))
            .and_then(|context| context.active_turn.as_mut())
        {
            if turn.id == permit.turn_id
                && turn.requests.get(&permit.request_id) == Some(&permit.id)
            {
                turn.requests.remove(&permit.request_id);
            }
        }
        self.permits.remove(&permit.id);
    }

    /// `id` must be resolved from a paired credential by the transport. IP is display context.
    pub fn register_client(
        &mut self,
        id: &str,
        label: &str,
        observed_ip: &str,
        now_ms: u64,
    ) -> Result<(), HubError> {
        check_token(id)?;
        check_text(label, 256)?;
        check_text(observed_ip, 128)?;
        if let Some(client) = self.clients.get_mut(id) {
            client.label = label.to_owned();
            client.observed_ip = observed_ip.to_owned();
            client.last_seen_ms = now_ms;
            return Ok(());
        }
        if self.clients.len() >= MAX_CLIENTS {
            return Err(HubError::RegistryFull);
        }
        self.clients.insert(
            id.to_owned(),
            Client {
                id: id.to_owned(),
                label: label.to_owned(),
                observed_ip: observed_ip.to_owned(),
                last_seen_ms: now_ms,
                activity: ClientActivity::Idle,
                contexts: BTreeMap::new(),
                closed_delegations: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub fn heartbeat(
        &mut self,
        id: &str,
        activity: ClientActivity,
        now_ms: u64,
    ) -> Result<(), HubError> {
        self.heartbeat_with_turns(id, activity, &[], now_ms)
    }

    /// Renew only live turns explicitly reported by the authenticated runtime owner.
    /// This extends affinity metadata, never an execution permit or a closed/expired turn.
    pub fn heartbeat_with_turns(
        &mut self,
        id: &str,
        activity: ClientActivity,
        active_turns: &[(&str, &str)],
        now_ms: u64,
    ) -> Result<(), HubError> {
        let mut contexts = BTreeSet::new();
        if active_turns.len() > MAX_CONTEXTS_PER_CLIENT {
            return Err(HubError::InvalidRequest {
                message: "too many active turn contexts".into(),
            });
        }
        for (context, turn) in active_turns {
            check_token(context)?;
            check_token(turn)?;
            if !contexts.insert(*context) {
                return Err(HubError::InvalidRequest {
                    message: "duplicate active turn context".into(),
                });
            }
        }
        self.expire_delegated_contexts(id, now_ms)?;
        let client = self.clients.get_mut(id).ok_or(HubError::UnknownClient)?;
        client.last_seen_ms = now_ms;
        client.activity = activity;
        for (context_id, turn_id) in active_turns {
            let Some(context) = client.contexts.get_mut(*context_id) else {
                continue;
            };
            if context
                .active_turn
                .as_ref()
                .is_some_and(|turn| turn.id == *turn_id)
                || context
                    .waiting_turn
                    .as_ref()
                    .is_some_and(|(turn, _)| turn == *turn_id)
            {
                if let Some(delegated) = &mut context.delegated {
                    delegated.last_seen_ms = now_ms;
                }
            }
            let Some(turn) = context.active_turn.as_mut().filter(|turn| {
                turn.id == *turn_id
                    && turn.lease.expires_at_ms > now_ms
                    && turn.lease.catalog_revision == self.store.revision
            }) else {
                // Delayed heartbeats may overlap prepare, finish or a newer turn.
                continue;
            };
            turn.lease.expires_at_ms = now_ms
                .saturating_add(self.store.catalog.policy.lease_ttl_ms)
                .max(turn.lease.expires_at_ms);
            if let Some(affinity) = context
                .lease
                .as_mut()
                .filter(|lease| lease.id == turn.lease.id)
            {
                affinity.expires_at_ms = turn.lease.expires_at_ms;
            }
        }
        Ok(())
    }

    /// Revocation discards runtime routing state and all advisory permits for this identity.
    pub fn remove_client(&mut self, id: &str) -> Result<(), HubError> {
        self.clients.remove(id).ok_or(HubError::UnknownClient)?;
        self.request_queue.retain(|ticket| ticket.client_id != id);
        self.permits
            .retain(|_, permit| permit.permit.client_id != id);
        Ok(())
    }

    pub fn save_selection(
        &mut self,
        id: &str,
        context_id: &str,
        selection: Selection,
    ) -> Result<(), HubError> {
        check_token(context_id)?;
        if context_id.starts_with("delegated-") {
            return Err(HubError::WrongOwner);
        }
        self.validate_selection(&selection)?;
        let client = self.clients.get_mut(id).ok_or(HubError::UnknownClient)?;
        if let Some(context) = client.contexts.get_mut(context_id) {
            if context.active_turn.is_some() || context.waiting_turn.is_some() {
                return Err(HubError::ActiveTurn);
            }
            context.selection = selection;
            context.lease = None;
        } else {
            if client.contexts.len() >= MAX_CONTEXTS_PER_CLIENT {
                return Err(HubError::RegistryFull);
            }
            client.contexts.insert(
                context_id.to_owned(),
                RouteContext {
                    delegated: None,
                    selection,
                    lease: None,
                    active_turn: None,
                    waiting_turn: None,
                    recent_closed_turns: VecDeque::new(),
                },
            );
        }
        Ok(())
    }

    pub fn begin_turn(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
        now_ms: u64,
    ) -> Result<RouteDecision, HubError> {
        self.begin_request(id, context_id, turn_id, RequestPurpose::UserTurn, now_ms)
    }

    pub fn begin_request(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
        purpose: RequestPurpose,
        now_ms: u64,
    ) -> Result<RouteDecision, HubError> {
        check_token(turn_id)?;
        self.expire_permits(now_ms);
        let context = self
            .checked_client(id, now_ms)?
            .contexts
            .get(context_id)
            .ok_or(HubError::SelectionRequired)?
            .clone();
        let selection = &context.selection;
        self.validate_selection(selection)?;
        if (purpose == RequestPurpose::Delegated) != context.delegated.is_some() {
            return Err(HubError::WrongOwner);
        }
        if let Some(delegated) = &context.delegated {
            if delegated.expired(now_ms, self.store.catalog.policy.heartbeat_stale_ms) {
                return Err(HubError::TurnClosed);
            }
            if Self::delegated_context_id(&delegated.parent, turn_id)? != context_id {
                return Err(HubError::WrongOwner);
            }
        }
        if context
            .recent_closed_turns
            .iter()
            .any(|closed| closed == turn_id)
        {
            return Err(HubError::TurnClosed);
        }
        if let Some(turn) = context.active_turn {
            if turn.id != turn_id || turn.purpose != purpose {
                return Err(HubError::ActiveTurn);
            }
            if turn.lease.expires_at_ms <= now_ms {
                return Err(HubError::LeaseExpired);
            }
            return Ok(RouteDecision::Assigned { lease: turn.lease });
        }
        if context
            .waiting_turn
            .as_ref()
            .is_some_and(|(waiting, waiting_purpose)| {
                waiting != turn_id || *waiting_purpose != purpose
            })
        {
            return Err(HubError::ActiveTurn);
        }
        let prior = context.lease.filter(|lease| {
            lease.catalog_revision == self.store.revision
                && lease.expires_at_ms > now_ms
                && lease.turns_remaining > 0
        });
        let (deployment, lease) = if let Some(lease) = prior {
            let deployment = self
                .store
                .catalog
                .deployments
                .iter()
                .find(|deployment| deployment.id == lease.deployment_id)
                .ok_or(HubError::LeaseExpired)?
                .clone();
            if let Some(reason) = self.unavailable_reason(&deployment, now_ms)? {
                return Ok(self.wait_for_route(id, context_id, turn_id, purpose, reason));
            }
            (deployment, Some(lease))
        } else {
            match self.select_deployment(selection, now_ms) {
                Ok(deployment) => (deployment, None),
                Err(RouteBlock::Wait(reason)) => {
                    return Ok(self.wait_for_route(id, context_id, turn_id, purpose, reason));
                }
                Err(RouteBlock::Failure(error)) => return Err(error),
            }
        };
        let mut lease = lease.unwrap_or_else(|| RouteLease {
            id: ulid::Ulid::new().to_string(),
            client_id: id.to_owned(),
            context_id: context_id.to_owned(),
            logical_model_id: deployment.logical_model_id.clone(),
            deployment_id: deployment.id,
            catalog_revision: self.store.revision,
            expires_at_ms: now_ms.saturating_add(self.store.catalog.policy.lease_ttl_ms),
            turns_remaining: selection.affinity_turns,
            advisory_only: true,
        });
        if purpose == RequestPurpose::UserTurn {
            lease.turns_remaining -= 1;
        }
        let context = self
            .clients
            .get_mut(id)
            .and_then(|client| client.contexts.get_mut(context_id))
            .ok_or(HubError::SelectionRequired)?;
        context.lease = Some(lease.clone());
        context.active_turn = Some(ActiveTurn {
            id: turn_id.to_owned(),
            purpose,
            lease: lease.clone(),
            requests: BTreeMap::new(),
        });
        context.waiting_turn = None;
        Ok(RouteDecision::Assigned { lease })
    }

    pub fn acquire_permit(
        &mut self,
        id: &str,
        context_id: &str,
        lease_id: &str,
        request_id: &str,
        now_ms: u64,
    ) -> Result<PermitDecision, HubError> {
        check_token(request_id)?;
        self.expire_permits(now_ms);
        let client = self.checked_client(id, now_ms)?;
        let context = client
            .contexts
            .get(context_id)
            .ok_or(HubError::SelectionRequired)?;
        let turn = context.active_turn.as_ref().ok_or(HubError::LeaseExpired)?;
        if turn.lease.id != lease_id {
            return Err(HubError::WrongOwner);
        }
        // An already-issued request can be acknowledged after a catalog change; no new one can start.
        if let Some(permit_id) = turn.requests.get(request_id) {
            return self
                .permits
                .get(permit_id)
                .map(|state| PermitDecision::Granted {
                    permit: state.permit.clone(),
                })
                .ok_or(HubError::PermitExpired);
        }
        self.validate_selection(&context.selection)?;
        if turn.lease.expires_at_ms <= now_ms {
            return Err(HubError::LeaseExpired);
        }
        if turn.requests.len() >= MAX_REQUESTS_PER_TURN {
            return Err(HubError::RegistryFull);
        }
        let deployment = self
            .store
            .catalog
            .deployments
            .iter()
            .find(|deployment| deployment.id == turn.lease.deployment_id)
            .ok_or(HubError::LeaseExpired)?
            .clone();
        if let Some(reason) = self.unavailable_reason(&deployment, now_ms)? {
            return Ok(PermitDecision::Waiting {
                reason,
                retry_after_ms: RETRY_AFTER_MS,
            });
        }
        let permit = ExecutionPermit {
            id: ulid::Ulid::new().to_string(),
            client_id: id.to_owned(),
            context_id: context_id.to_owned(),
            lease_id: lease_id.to_owned(),
            request_id: request_id.to_owned(),
            turn_id: turn.id.clone(),
            catalog_revision: turn.lease.catalog_revision,
            logical_model_id: deployment.logical_model_id.clone(),
            deployment_id: deployment.id.clone(),
            provider_profile: deployment.provider_profile.clone(),
            expires_at_ms: now_ms.saturating_add(self.store.catalog.policy.permit_ttl_ms),
            advisory_only: true,
        };
        self.permits.insert(
            permit.id.clone(),
            PermitState {
                permit: permit.clone(),
                capacity_pool_id: deployment.capacity_pool_id,
            },
        );
        self.clients
            .get_mut(id)
            .and_then(|client| client.contexts.get_mut(context_id))
            .and_then(|context| context.active_turn.as_mut())
            .ok_or(HubError::LeaseExpired)?
            .requests
            .insert(request_id.to_owned(), permit.id.clone());
        Ok(PermitDecision::Granted { permit })
    }

    /// Idempotent release by the authenticated owner, including after expiry or revision changes.
    pub fn release_permit(
        &mut self,
        id: &str,
        context_id: &str,
        permit_id: &str,
    ) -> Result<bool, HubError> {
        let client = self.clients.get(id).ok_or(HubError::UnknownClient)?;
        if let Some(permit) = self.permits.get(permit_id) {
            if permit.permit.client_id != id || permit.permit.context_id != context_id {
                return Err(HubError::WrongOwner);
            }
            self.permits.remove(permit_id);
            return Ok(true);
        }
        if client
            .contexts
            .get(context_id)
            .and_then(|context| context.active_turn.as_ref())
            .is_some_and(|turn| turn.requests.values().any(|value| value == permit_id))
        {
            return Ok(false);
        }
        Err(HubError::PermitExpired)
    }

    pub fn finish_turn(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
    ) -> Result<(), HubError> {
        self.close_turn(id, context_id, turn_id, false)
    }

    pub fn cancel_turn(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
    ) -> Result<(), HubError> {
        self.close_turn(id, context_id, turn_id, true)
    }

    pub fn deployment_rows(&self, now_ms: u64) -> Vec<DeploymentRow> {
        self.store
            .catalog
            .deployments
            .iter()
            .map(|deployment| DeploymentRow {
                deployment: deployment.clone(),
                availability: self.availability(deployment, now_ms),
                model_present: self.model_present(&deployment.id, now_ms),
                last_observed_ms: self
                    .observations
                    .get(&deployment.id)
                    .map(|value| value.observed_at_ms),
                active_permits: self
                    .permits
                    .values()
                    .filter(|state| {
                        state.permit.deployment_id == deployment.id
                            && state.permit.expires_at_ms > now_ms
                    })
                    .count() as u32,
            })
            .collect()
    }

    pub fn client_rows(&self, now_ms: u64) -> Vec<ClientRow> {
        self.clients
            .values()
            .map(|client| ClientRow {
                id: client.id.clone(),
                label: client.label.clone(),
                observed_ip: client.observed_ip.clone(),
                last_seen_ms: client.last_seen_ms,
                connection: if self.is_stale(client, now_ms) {
                    ClientConnection::Stale
                } else {
                    ClientConnection::Connected
                },
                activity: client.activity.clone(),
                contexts: client
                    .contexts
                    .iter()
                    .filter(|(_, context)| context.delegated.is_none())
                    .map(|(id, context)| RouteContextRow {
                        context_id: id.clone(),
                        reviewed_revision: context.selection.reviewed_revision,
                        review_required: context.selection.reviewed_revision != self.store.revision,
                        active_model_id: context
                            .active_turn
                            .as_ref()
                            .map(|turn| turn.lease.logical_model_id.clone()),
                    })
                    .collect(),
            })
            .collect()
    }

    fn close_turn(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
        cancel: bool,
    ) -> Result<(), HubError> {
        let client = self.clients.get_mut(id).ok_or(HubError::UnknownClient)?;
        let context = client
            .contexts
            .get_mut(context_id)
            .ok_or(HubError::SelectionRequired)?;
        if context
            .recent_closed_turns
            .iter()
            .any(|closed| closed == turn_id)
        {
            return Ok(());
        }
        let purpose = if context
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.id == turn_id)
        {
            context
                .active_turn
                .take()
                .expect("matching active turn")
                .purpose
        } else if context
            .waiting_turn
            .as_ref()
            .is_some_and(|(id, _)| id == turn_id)
        {
            context
                .waiting_turn
                .take()
                .expect("matching waiting turn")
                .1
        } else {
            return Err(HubError::WrongOwner);
        };
        self.permits.retain(|_, state| {
            state.permit.client_id != id || state.permit.context_id != context_id
        });
        self.request_queue.retain(|ticket| {
            ticket.client_id != id || ticket.context_id != context_id || ticket.turn_id != turn_id
        });
        if cancel && purpose == RequestPurpose::UserTurn {
            context.lease = None;
        }
        context.recent_closed_turns.push_back(turn_id.to_owned());
        if context.recent_closed_turns.len() > MAX_RECENT_TURNS {
            context.recent_closed_turns.pop_front();
        }
        Ok(())
    }

    fn validate_selection(&self, selection: &Selection) -> Result<(), HubError> {
        if selection.reviewed_revision != self.store.revision {
            return Err(HubError::ReviewRequired {
                current_revision: self.store.revision,
            });
        }
        let selected: BTreeSet<&str> = selection
            .allowed_model_ids
            .iter()
            .map(String::as_str)
            .collect();
        if selected.is_empty()
            || selected.len() > 128
            || selected.len() != selection.allowed_model_ids.len()
            || selection.affinity_turns == 0
            || selection.affinity_turns > 100
            || selection.required_capabilities.len() > 32
            || selection
                .required_capabilities
                .iter()
                .any(|capability| !valid_token(capability, 64))
            || selection
                .allowed_model_ids
                .iter()
                .any(|id| !valid_token(id, 128))
            || selection
                .preferred_model_id
                .as_ref()
                .is_some_and(|preferred| !selected.contains(preferred.as_str()))
        {
            return Err(HubError::InvalidSelection {
                message:
                    "select 1–128 unique models, a selected preference, and 1–100 affinity turns"
                        .to_owned(),
            });
        }
        let mut compatible = false;
        for id in &selection.allowed_model_ids {
            let model = self
                .store
                .catalog
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| HubError::ModelRemoved {
                    model_id: id.clone(),
                })?;
            if selection
                .required_capabilities
                .is_subset(&model.capabilities)
            {
                compatible = true;
            }
        }
        if !compatible {
            return Err(HubError::CapabilityMismatch);
        }
        if selection.wait_policy == WaitPolicy::WaitForPreferred {
            let preferred = selection
                .preferred_model_id
                .as_ref()
                .unwrap_or(&selection.allowed_model_ids[0]);
            if self
                .store
                .catalog
                .models
                .iter()
                .find(|model| &model.id == preferred)
                .is_none_or(|model| {
                    !selection
                        .required_capabilities
                        .is_subset(&model.capabilities)
                })
            {
                return Err(HubError::CapabilityMismatch);
            }
        }
        Ok(())
    }

    fn select_deployment(
        &self,
        selection: &Selection,
        now_ms: u64,
    ) -> Result<Deployment, RouteBlock> {
        let preferred = selection
            .preferred_model_id
            .as_ref()
            .unwrap_or(&selection.allowed_model_ids[0]);
        let mut ids = vec![preferred];
        if selection.wait_policy == WaitPolicy::AnyAllowed {
            ids.extend(
                selection
                    .allowed_model_ids
                    .iter()
                    .filter(|id| *id != preferred),
            );
        }
        let mut first_reason = None;
        let mut first_error = None;
        for id in ids {
            let compatible = self.store.catalog.models.iter().any(|model| {
                &model.id == id
                    && selection
                        .required_capabilities
                        .is_subset(&model.capabilities)
            });
            if !compatible {
                continue;
            }
            for deployment in self
                .store
                .catalog
                .deployments
                .iter()
                .filter(|deployment| &deployment.logical_model_id == id)
            {
                match self.unavailable_reason(deployment, now_ms) {
                    Ok(None) => return Ok(deployment.clone()),
                    Ok(Some(reason)) => {
                        first_reason.get_or_insert(reason);
                    }
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }
        match first_reason {
            Some(reason) => Err(RouteBlock::Wait(reason)),
            None => Err(RouteBlock::Failure(
                first_error.unwrap_or(HubError::DeploymentRequired),
            )),
        }
    }

    fn wait_for_route(
        &mut self,
        id: &str,
        context_id: &str,
        turn_id: &str,
        purpose: RequestPurpose,
        reason: WaitReason,
    ) -> RouteDecision {
        if let Some(context) = self
            .clients
            .get_mut(id)
            .and_then(|client| client.contexts.get_mut(context_id))
        {
            context.waiting_turn = Some((turn_id.to_owned(), purpose));
        }
        RouteDecision::Waiting {
            reason,
            retry_after_ms: RETRY_AFTER_MS,
        }
    }

    fn unavailable_reason(
        &self,
        deployment: &Deployment,
        now_ms: u64,
    ) -> Result<Option<WaitReason>, HubError> {
        match self.availability(deployment, now_ms) {
            Availability::Maintenance => return Ok(Some(WaitReason::Maintenance)),
            Availability::ModelMissing => {
                return Err(HubError::DeploymentModelMissing {
                    deployment_id: deployment.id.clone(),
                });
            }
            // Provider-reported load state is observational. A listed model may load
            // on its first request; only the provider decides whether/how to do that.
            _ if self.model_present(&deployment.id, now_ms) => {}
            _ => return Ok(Some(WaitReason::TargetUnavailable)),
        }
        let active: Vec<&PermitState> = self
            .permits
            .values()
            .filter(|state| state.permit.expires_at_ms > now_ms)
            .collect();
        if active.len() >= self.store.catalog.policy.global_max_concurrent as usize {
            return Ok(Some(WaitReason::GlobalCapacity));
        }
        let pool = self
            .store
            .catalog
            .capacity_pools
            .iter()
            .find(|pool| pool.id == deployment.capacity_pool_id)
            .ok_or(HubError::DeploymentRequired)?;
        if active
            .iter()
            .filter(|state| state.capacity_pool_id == pool.id)
            .count()
            >= pool.max_concurrent as usize
        {
            return Ok(Some(WaitReason::Busy));
        }
        Ok(None)
    }

    fn availability(&self, deployment: &Deployment, now_ms: u64) -> Availability {
        if !deployment.enabled {
            return Availability::Maintenance;
        }
        self.fresh_observation(&deployment.id, now_ms)
            .map_or(Availability::Unknown, |observation| {
                observation.availability.clone()
            })
    }

    fn model_present(&self, deployment_id: &str, now_ms: u64) -> bool {
        self.fresh_observation(deployment_id, now_ms)
            .is_some_and(|observation| observation.model_present)
    }

    fn fresh_observation(&self, deployment_id: &str, now_ms: u64) -> Option<&Observation> {
        self.observations.get(deployment_id).filter(|observation| {
            now_ms.saturating_sub(observation.observed_at_ms)
                < self.store.catalog.policy.heartbeat_stale_ms
        })
    }

    fn checked_client(&self, id: &str, now_ms: u64) -> Result<&Client, HubError> {
        let client = self.clients.get(id).ok_or(HubError::UnknownClient)?;
        if self.is_stale(client, now_ms) {
            return Err(HubError::StaleClient);
        }
        Ok(client)
    }

    fn is_stale(&self, client: &Client, now_ms: u64) -> bool {
        now_ms.saturating_sub(client.last_seen_ms) >= self.store.catalog.policy.heartbeat_stale_ms
    }

    fn expire_permits(&mut self, now_ms: u64) {
        self.permits
            .retain(|_, state| state.permit.expires_at_ms > now_ms);
    }
}

fn check_text(value: &str, max: usize) -> Result<(), HubError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(HubError::InvalidRequest {
            message: "invalid empty, oversized or control-character text".to_owned(),
        })
    } else {
        Ok(())
    }
}

fn check_token(value: &str) -> Result<(), HubError> {
    if valid_token(value, 128) {
        Ok(())
    } else {
        Err(HubError::InvalidRequest {
            message: "identity must contain 1–128 ASCII letters, digits, dash, underscore or dot"
                .to_owned(),
        })
    }
}
