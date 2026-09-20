use moyai_hub::{catalog::*, core::HubCore, protocol::*};
use std::collections::BTreeSet;

#[test]
fn legacy_catalog_upgrades_without_assuming_a_standard_and_explicit_choice_is_validated() {
    let mut store = fixture().catalog_store().clone();
    store.schema_version = 1;
    let legacy = serde_json::to_string(&store).unwrap();
    assert!(!legacy.contains("team_default_model_id"));
    let mut reopened: CatalogStore = serde_json::from_str(&legacy).unwrap();
    assert!(reopened.reconcile_software("0.1.0", 20).unwrap());
    assert_eq!(reopened.schema_version, SCHEMA_VERSION);
    assert_eq!(reopened.catalog.team_default_model_id, None);
    let mut selected = reopened.catalog.clone();
    selected.team_default_model_id = Some("model-b".into());
    reopened.replace_catalog(selected.clone(), 21).unwrap();
    assert!(reopened.changes.last().unwrap().policy_changed);
    assert_eq!(
        serde_json::from_str::<CatalogStore>(&serde_json::to_string(&reopened).unwrap()).unwrap(),
        reopened
    );
    selected.team_default_model_id = Some("missing".into());
    assert!(selected.validate().is_err());
    selected.team_default_model_id = Some("model-b".into());
    selected.models[1].capabilities.clear();
    assert!(selected.validate().is_err());
}

fn fixture() -> HubCore {
    let mut store = CatalogStore::empty("0.1.0");
    store
        .replace_catalog(
            Catalog {
                team_default_model_id: None,
                models: vec![
                    model("model-a", &["tools"]),
                    model("model-b", &["tools", "vision"]),
                ],
                deployments: vec![
                    deployment("host-a", "model-a", "pool-a"),
                    deployment("host-b", "model-b", "pool-b"),
                ],
                capacity_pools: vec![
                    CapacityPool {
                        id: "pool-a".into(),
                        label: "GPU A".into(),
                        max_concurrent: 1,
                    },
                    CapacityPool {
                        id: "pool-b".into(),
                        label: "GPU B".into(),
                        max_concurrent: 1,
                    },
                ],
                policy: CatalogPolicy {
                    global_max_concurrent: 2,
                    permit_ttl_ms: 1000,
                    heartbeat_stale_ms: 10_000,
                    lease_ttl_ms: 5000,
                },
            },
            0,
        )
        .unwrap();
    let mut hub = HubCore::new(store).unwrap();
    for host in ["host-a", "host-b"] {
        hub.observe_deployment(host, Availability::Ready, 0)
            .unwrap();
    }
    for client in ["client-a", "client-b", "client-c"] {
        hub.register_client(client, client, "192.0.2.1", 0).unwrap();
        hub.save_selection(client, "main", selection(hub.catalog_store().revision))
            .unwrap();
    }
    hub
}

fn model(id: &str, capabilities: &[&str]) -> LogicalModel {
    LogicalModel {
        id: id.into(),
        label: id.into(),
        capabilities: capabilities.iter().map(|value| value.to_string()).collect(),
    }
}

fn deployment(id: &str, model: &str, pool: &str) -> Deployment {
    Deployment {
        id: id.into(),
        logical_model_id: model.into(),
        capacity_pool_id: pool.into(),
        label: id.into(),
        endpoint: "http://127.0.0.1:1234/v1".into(),
        provider_model: model.into(),
        provider_profile: "openai_compatible_chat".into(),
        enabled: true,
    }
}

fn selection(revision: u64) -> Selection {
    Selection {
        allowed_model_ids: vec!["model-a".into()],
        preferred_model_id: Some("model-a".into()),
        required_capabilities: BTreeSet::from(["tools".into()]),
        wait_policy: WaitPolicy::WaitForPreferred,
        affinity_turns: 2,
        reviewed_revision: revision,
    }
}

#[test]
fn generic_listing_presence_allows_attempt_without_claiming_load_readiness() {
    let mut hub = fixture();
    hub.observe_deployment("host-a", Availability::Unknown, 10)
        .unwrap();
    assert!(!hub.deployment_rows(11)[0].model_present);
    assert!(matches!(
        hub.begin_turn("client-a", "main", "unknown", 11).unwrap(),
        RouteDecision::Waiting {
            reason: WaitReason::TargetUnavailable,
            ..
        }
    ));
    hub.cancel_turn("client-a", "main", "unknown").unwrap();
    hub.observe_present_deployment("host-a", Availability::Unknown, 12)
        .unwrap();
    assert_eq!(
        hub.deployment_rows(13)[0].availability,
        Availability::Unknown
    );
    assert!(hub.deployment_rows(13)[0].model_present);
    assert!(matches!(
        hub.begin_turn("client-a", "main", "listed", 13).unwrap(),
        RouteDecision::Assigned { .. }
    ));
    hub.cancel_turn("client-a", "main", "listed").unwrap();
    hub.register_client("client-a", "client-a", "192.0.2.1", 10_020)
        .unwrap();
    assert!(hub.needs_presence_refresh("host-a", 10_020));
    assert!(!hub.deployment_rows(10_020)[0].model_present);
    assert!(matches!(
        hub.begin_turn("client-a", "main", "stale-presence", 10_020)
            .unwrap(),
        RouteDecision::Waiting {
            reason: WaitReason::TargetUnavailable,
            ..
        }
    ));
}

#[test]
fn native_listing_allows_request_before_provider_load() {
    for status in [Availability::JitPending, Availability::Unknown] {
        let mut hub = fixture();
        let mut catalog = hub.catalog_store().catalog.clone();
        catalog.deployments[0].provider_profile = "lmstudio_chat".into();
        let revision = hub.replace_catalog(catalog, 10).unwrap();
        hub.save_selection("client-a", "main", selection(revision))
            .unwrap();
        hub.observe_present_deployment("host-a", status.clone(), 12)
            .unwrap();

        let route = lease(hub.begin_turn("client-a", "main", "listed", 13).unwrap());
        let request = permit(
            hub.acquire_permit("client-a", "main", &route.id, "request", 14)
                .unwrap(),
        );
        assert_eq!(request.provider_profile, "lmstudio_chat");
        assert_eq!(request.deployment_id, "host-a");
        assert_eq!(hub.deployment_rows(14)[0].availability, status);
        assert!(hub.deployment_rows(14)[0].model_present);
        assert_eq!(hub.catalog_store().revision, revision);
    }
}

#[test]
fn listed_model_can_be_maintained_and_old_snapshots_do_not_infer_presence() {
    let mut hub = fixture();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.deployments[0].enabled = false;
    let revision = hub.replace_catalog(catalog, 10).unwrap();
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    hub.observe_present_deployment("host-a", Availability::JitPending, 12)
        .unwrap();
    let row = &hub.deployment_rows(13)[0];
    assert!(row.model_present);
    assert_eq!(row.availability, Availability::Maintenance);
    assert_eq!(
        hub.begin_turn("client-a", "main", "maintained", 13)
            .unwrap(),
        RouteDecision::Waiting {
            reason: WaitReason::Maintenance,
            retry_after_ms: 1000,
        }
    );

    let mut older = serde_json::to_value(row).unwrap();
    older.as_object_mut().unwrap().remove("model_present");
    let older: DeploymentRow = serde_json::from_value(older).unwrap();
    assert!(!older.model_present);
    assert_eq!(older.last_observed_ms, Some(12));
}

#[test]
fn gateway_claim_rechecks_revision_cancellation_and_expiry() {
    let mut hub = fixture();
    let route = lease(hub.begin_turn("client-a", "main", "turn", 10).unwrap());
    let ticket = permit(
        hub.acquire_permit("client-a", "main", &route.id, "request", 11)
            .unwrap(),
    );
    assert_eq!(
        hub.validate_execution_permit(&ticket.id, 12)
            .unwrap()
            .turn_id,
        "turn"
    );
    assert!(matches!(
        hub.validate_execution_permit(&ticket.id, 1011),
        Err(HubError::PermitExpired)
    ));
    let mut changed = hub.catalog_store().catalog.clone();
    changed.models[0].label = "Changed".into();
    hub.replace_catalog(changed, 20).unwrap();
    assert!(matches!(
        hub.validate_execution_permit(&ticket.id, 21),
        Err(HubError::ReviewRequired { .. })
    ));
    hub.cancel_turn("client-a", "main", "turn").unwrap();
    assert!(matches!(
        hub.validate_execution_permit(&ticket.id, 22),
        Err(HubError::PermitExpired)
    ));
}

fn lease(decision: RouteDecision) -> RouteLease {
    match decision {
        RouteDecision::Assigned { lease } => lease,
        other => panic!("expected assignment, got {other:?}"),
    }
}

fn permit(decision: PermitDecision) -> ExecutionPermit {
    match decision {
        PermitDecision::Granted { permit } => permit,
        other => panic!("expected permit, got {other:?}"),
    }
}

#[test]
fn revision_survives_reload_and_software_change_once() {
    let hub = fixture();
    let bytes = serde_json::to_vec(hub.catalog_store()).unwrap();
    let mut loaded: CatalogStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(loaded.hub_id, hub.catalog_store().hub_id);
    let old = loaded.revision;
    assert!(!loaded.reconcile_software("0.1.0", 10).unwrap());
    assert_eq!(loaded.revision, old);
    assert!(loaded.reconcile_software("0.2.0", 20).unwrap());
    assert_eq!(loaded.revision, old + 1);
    assert!(loaded.changes.last().unwrap().software_changed);
    let mut reloaded: CatalogStore =
        serde_json::from_slice(&serde_json::to_vec(&loaded).unwrap()).unwrap();
    assert!(!reloaded.reconcile_software("0.2.0", 30).unwrap());
    assert_eq!(reloaded.revision, old + 1);
    assert_eq!(
        reloaded
            .replace_catalog(reloaded.catalog.clone(), 40)
            .unwrap(),
        old + 1
    );
}

#[test]
fn catalog_mutation_is_atomic_and_revision_overflow_fails_closed() {
    let hub = fixture();
    let mut store = hub.catalog_store().clone();
    let before = store.clone();
    let mut invalid = store.catalog.clone();
    invalid.deployments[0].logical_model_id = "missing".into();
    assert!(store.replace_catalog(invalid, 1).is_err());
    assert_eq!(store, before);
    store.revision = u64::MAX;
    let mut changed = store.catalog.clone();
    changed.models[0].label = "New label".into();
    assert_eq!(
        store.replace_catalog(changed, 1),
        Err(HubError::RevisionExhausted)
    );
    assert_eq!(store.catalog, before.catalog);
    assert_eq!(
        store.reconcile_software("next", 1),
        Err(HubError::RevisionExhausted)
    );
    assert_eq!(store.software_version, before.software_version);
}

#[test]
fn observations_do_not_change_review_revision_and_expire_to_unknown() {
    let mut hub = fixture();
    let revision = hub.catalog_store().revision;
    for state in [
        Availability::Unknown,
        Availability::Offline,
        Availability::ModelMissing,
        Availability::JitPending,
        Availability::Ready,
    ] {
        hub.observe_deployment("host-a", state.clone(), 10).unwrap();
        assert_eq!(hub.deployment_rows(10)[0].availability, state);
        assert_eq!(hub.catalog_store().revision, revision);
    }
    assert_eq!(
        hub.deployment_rows(10_010)[0].availability,
        Availability::Unknown
    );
    let mut changed = hub.catalog_store().catalog.clone();
    changed.deployments[0].endpoint = "http://127.0.0.1:4321/v1".into();
    hub.replace_catalog(changed, 20).unwrap();
    assert_eq!(
        hub.deployment_rows(20)[0].availability,
        Availability::Unknown
    );
}

#[test]
fn only_selected_capable_models_can_be_allocated() {
    let mut hub = fixture();
    hub.observe_deployment("host-a", Availability::Offline, 0)
        .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "t1", 0).unwrap(),
        RouteDecision::Waiting {
            reason: WaitReason::TargetUnavailable,
            retry_after_ms: 1000
        }
    );
    hub.cancel_turn("client-a", "main", "t1").unwrap();
    let mut selected = selection(hub.catalog_store().revision);
    selected.allowed_model_ids.push("model-b".into());
    selected.wait_policy = WaitPolicy::AnyAllowed;
    selected.required_capabilities.insert("vision".into());
    hub.save_selection("client-a", "main", selected).unwrap();
    assert_eq!(
        lease(hub.begin_turn("client-a", "main", "t2", 0).unwrap()).logical_model_id,
        "model-b"
    );
    let mut invalid = selection(hub.catalog_store().revision);
    invalid.required_capabilities.insert("vision".into());
    assert_eq!(
        hub.save_selection("client-b", "main", invalid),
        Err(HubError::CapabilityMismatch)
    );
    let mut invalid = selection(hub.catalog_store().revision);
    invalid.allowed_model_ids.clear();
    assert!(matches!(
        hub.save_selection("client-b", "main", invalid),
        Err(HubError::InvalidSelection { .. })
    ));
}

#[test]
fn removal_and_capability_changes_require_review_then_reconfiguration() {
    let mut hub = fixture();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models.retain(|model| model.id != "model-a");
    catalog
        .deployments
        .retain(|deployment| deployment.logical_model_id != "model-a");
    let revision = hub.replace_catalog(catalog, 1).unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "t1", 1),
        Err(HubError::ReviewRequired {
            current_revision: revision
        })
    );
    assert_eq!(
        hub.save_selection("client-a", "main", selection(revision)),
        Err(HubError::ModelRemoved {
            model_id: "model-a".into()
        })
    );
    assert_eq!(
        hub.catalog_store()
            .changes
            .last()
            .unwrap()
            .removed_model_ids,
        vec!["model-a"]
    );
    assert!(hub.client_rows(1).iter().all(|client| {
        client
            .contexts
            .iter()
            .all(|context| context.review_required)
    }));
}

#[test]
fn affinity_counts_user_turn_once_while_permits_count_requests() {
    let mut hub = fixture();
    let first = lease(hub.begin_turn("client-a", "main", "user-1", 0).unwrap());
    assert_eq!(first.turns_remaining, 1);
    assert_eq!(
        lease(hub.begin_turn("client-a", "main", "user-1", 0).unwrap()),
        first
    );
    let request = permit(
        hub.acquire_permit("client-a", "main", &first.id, "request-1", 0)
            .unwrap(),
    );
    assert_eq!(
        permit(
            hub.acquire_permit("client-a", "main", &first.id, "request-1", 0)
                .unwrap()
        ),
        request
    );
    hub.release_permit("client-a", "main", &request.id).unwrap();
    assert_eq!(
        hub.release_permit("client-a", "main", &request.id).unwrap(),
        false
    );
    assert_eq!(
        hub.acquire_permit("client-a", "main", &first.id, "request-1", 0),
        Err(HubError::PermitExpired)
    );
    let second_request = permit(
        hub.acquire_permit("client-a", "main", &first.id, "request-2", 0)
            .unwrap(),
    );
    assert_ne!(second_request.id, request.id);
    assert_eq!(
        lease(hub.begin_turn("client-a", "main", "user-1", 0).unwrap()).turns_remaining,
        1
    );
    hub.finish_turn("client-a", "main", "user-1").unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "user-1", 0),
        Err(HubError::TurnClosed)
    );
    let second = lease(hub.begin_turn("client-a", "main", "user-2", 0).unwrap());
    assert_eq!(second.id, first.id);
    assert_eq!(second.turns_remaining, 0);
    hub.finish_turn("client-a", "main", "user-2").unwrap();
    let third = lease(hub.begin_turn("client-a", "main", "user-3", 0).unwrap());
    assert_ne!(third.id, first.id);
    assert_eq!(third.turns_remaining, 1);
}

#[test]
fn preparation_shares_permits_and_preserves_user_turn_affinity() {
    let mut hub = fixture();
    let user = lease(hub.begin_turn("client-a", "main", "user-1", 0).unwrap());
    hub.finish_turn("client-a", "main", "user-1").unwrap();
    let preparation = lease(
        hub.begin_request(
            "client-a",
            "main",
            "prepare-1",
            RequestPurpose::Preparation,
            1,
        )
        .unwrap(),
    );
    assert_eq!(preparation.id, user.id);
    assert_eq!(preparation.turns_remaining, 1);
    assert_eq!(
        hub.begin_turn("client-a", "main", "prepare-1", 1),
        Err(HubError::ActiveTurn)
    );
    let granted = permit(
        hub.acquire_permit("client-a", "main", &preparation.id, "prepare-request", 1)
            .unwrap(),
    );
    assert!(matches!(
        hub.begin_turn("client-b", "main", "other-user", 1).unwrap(),
        RouteDecision::Waiting { .. }
    ));
    hub.release_permit("client-a", "main", &granted.id).unwrap();
    hub.cancel_turn("client-a", "main", "prepare-1").unwrap();
    let second = lease(hub.begin_turn("client-a", "main", "user-2", 2).unwrap());
    assert_eq!(
        second.id, user.id,
        "cancelling preparation retains the user's affinity"
    );
    assert_eq!(second.turns_remaining, 0);
    hub.finish_turn("client-a", "main", "user-2").unwrap();
    let fresh = lease(
        hub.begin_request(
            "client-a",
            "main",
            "prepare-2",
            RequestPurpose::Preparation,
            3,
        )
        .unwrap(),
    );
    assert_ne!(fresh.id, user.id);
    assert_eq!(fresh.turns_remaining, 2);
    assert_eq!(
        lease(
            hub.begin_request(
                "client-a",
                "main",
                "prepare-2",
                RequestPurpose::Preparation,
                3
            )
            .unwrap()
        ),
        fresh
    );
    hub.finish_turn("client-a", "main", "prepare-2").unwrap();
    let next = lease(hub.begin_turn("client-a", "main", "user-3", 4).unwrap());
    assert_eq!(next.id, fresh.id);
    assert_eq!(next.turns_remaining, 1);
}

#[test]
fn waiting_preparation_cannot_change_purpose_and_remains_subject_to_review() {
    let mut hub = fixture();
    let active = lease(hub.begin_turn("client-a", "main", "active", 0).unwrap());
    let occupied = permit(
        hub.acquire_permit("client-a", "main", &active.id, "request", 0)
            .unwrap(),
    );
    assert!(matches!(
        hub.begin_request(
            "client-b",
            "main",
            "waiting",
            RequestPurpose::Preparation,
            1
        )
        .unwrap(),
        RouteDecision::Waiting { .. }
    ));
    assert_eq!(
        hub.begin_turn("client-b", "main", "waiting", 1),
        Err(HubError::ActiveTurn)
    );
    hub.release_permit("client-a", "main", &occupied.id)
        .unwrap();
    let ready = lease(
        hub.begin_request(
            "client-b",
            "main",
            "waiting",
            RequestPurpose::Preparation,
            2,
        )
        .unwrap(),
    );
    assert_eq!(ready.turns_remaining, 2);
    hub.finish_turn("client-b", "main", "waiting").unwrap();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models[0].label = "changed model".into();
    hub.replace_catalog(catalog, 3).unwrap();
    assert!(matches!(
        hub.begin_request(
            "client-b",
            "main",
            "review-needed",
            RequestPurpose::Preparation,
            3
        ),
        Err(HubError::ReviewRequired { .. })
    ));
}

#[test]
fn leases_do_not_reserve_capacity_and_wait_can_be_canceled() {
    let mut hub = fixture();
    let a = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    let b = lease(hub.begin_turn("client-b", "main", "t1", 0).unwrap());
    assert_eq!(hub.deployment_rows(0)[0].active_permits, 0);
    let active = permit(
        hub.acquire_permit("client-a", "main", &a.id, "req1", 0)
            .unwrap(),
    );
    assert_eq!(
        hub.acquire_permit("client-b", "main", &b.id, "req1", 0)
            .unwrap(),
        PermitDecision::Waiting {
            reason: WaitReason::Busy,
            retry_after_ms: 1000
        }
    );
    assert_eq!(
        hub.begin_turn("client-c", "main", "waiting", 0).unwrap(),
        RouteDecision::Waiting {
            reason: WaitReason::Busy,
            retry_after_ms: 1000
        }
    );
    hub.cancel_turn("client-c", "main", "waiting").unwrap();
    assert_eq!(
        hub.begin_turn("client-c", "main", "waiting", 0),
        Err(HubError::TurnClosed)
    );
    hub.release_permit("client-a", "main", &active.id).unwrap();
    assert!(matches!(
        hub.acquire_permit("client-b", "main", &b.id, "req1", 0)
            .unwrap(),
        PermitDecision::Granted { .. }
    ));
    hub.cancel_turn("client-b", "main", "t1").unwrap();
    assert_eq!(hub.deployment_rows(0)[0].active_permits, 0);
}

#[test]
fn global_capacity_expiry_and_owner_checks_are_independent() {
    let mut hub = fixture();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.policy.global_max_concurrent = 1;
    let revision = hub.replace_catalog(catalog, 0).unwrap();
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    let mut b_selection = selection(revision);
    b_selection.allowed_model_ids = vec!["model-b".into()];
    b_selection.preferred_model_id = Some("model-b".into());
    hub.save_selection("client-b", "main", b_selection).unwrap();
    let a = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    let b = lease(hub.begin_turn("client-b", "main", "t1", 0).unwrap());
    let first = permit(
        hub.acquire_permit("client-a", "main", &a.id, "r1", 0)
            .unwrap(),
    );
    assert_eq!(
        hub.release_permit("client-b", "main", &first.id),
        Err(HubError::WrongOwner)
    );
    assert_eq!(
        hub.acquire_permit("client-b", "main", &a.id, "r1", 0),
        Err(HubError::WrongOwner)
    );
    assert_eq!(
        hub.acquire_permit("client-b", "main", &b.id, "r1", 1)
            .unwrap(),
        PermitDecision::Waiting {
            reason: WaitReason::GlobalCapacity,
            retry_after_ms: 1000
        }
    );
    assert!(matches!(
        hub.acquire_permit("client-b", "main", &b.id, "r1", 1000)
            .unwrap(),
        PermitDecision::Granted { .. }
    ));
    assert_eq!(
        hub.acquire_permit("client-a", "main", &a.id, "r1", 1000),
        Err(HubError::PermitExpired)
    );
}

#[test]
fn revision_change_keeps_issued_permit_but_rejects_new_requests() {
    let mut hub = fixture();
    let route = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    let active = permit(
        hub.acquire_permit("client-a", "main", &route.id, "r1", 0)
            .unwrap(),
    );
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models[0].capabilities.insert("vision".into());
    let revision = hub.replace_catalog(catalog, 1).unwrap();
    assert_eq!(
        permit(
            hub.acquire_permit("client-a", "main", &route.id, "r1", 1)
                .unwrap()
        ),
        active
    );
    assert_eq!(
        hub.acquire_permit("client-a", "main", &route.id, "r2", 1),
        Err(HubError::ReviewRequired {
            current_revision: revision
        })
    );
    assert_eq!(hub.deployment_rows(1)[0].active_permits, 1);
    hub.release_permit("client-a", "main", &active.id).unwrap();
    hub.finish_turn("client-a", "main", "t1").unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "t2", 1),
        Err(HubError::ReviewRequired {
            current_revision: revision
        })
    );
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    let next = lease(hub.begin_turn("client-a", "main", "t2", 1).unwrap());
    assert_ne!(next.id, route.id);
    assert_eq!(next.catalog_revision, revision);
}

#[test]
fn heartbeat_stale_is_separate_from_activity_and_ip_is_not_identity() {
    let mut hub = fixture();
    hub.heartbeat("client-a", ClientActivity::Running, 0)
        .unwrap();
    let rows = hub.client_rows(10_000);
    let row = rows.iter().find(|row| row.id == "client-a").unwrap();
    assert_eq!(row.connection, ClientConnection::Stale);
    assert_eq!(row.activity, ClientActivity::Running);
    assert_eq!(
        hub.begin_turn("client-a", "main", "t1", 10_000),
        Err(HubError::StaleClient)
    );
    hub.register_client("client-a", "Renamed", "192.0.2.99", 10_001)
        .unwrap();
    let rows = hub.client_rows(10_001);
    assert_eq!(rows.len(), 3);
    let row = rows.iter().find(|row| row.id == "client-a").unwrap();
    assert_eq!(row.observed_ip, "192.0.2.99");
    assert_eq!(row.connection, ClientConnection::Connected);
    assert!(!row.contexts[0].review_required);
}

#[test]
fn explicit_maintenance_and_connection_failures_remain_routing_gates() {
    let mut hub = fixture();
    for (index, (status, reason)) in [
        (Availability::Maintenance, WaitReason::Maintenance),
        (Availability::Offline, WaitReason::TargetUnavailable),
        (Availability::Unknown, WaitReason::TargetUnavailable),
    ]
    .into_iter()
    .enumerate()
    {
        hub.observe_deployment("host-a", status, 0).unwrap();
        let turn = format!("turn-{index}");
        assert_eq!(
            hub.begin_turn("client-a", "main", &turn, 0).unwrap(),
            RouteDecision::Waiting {
                reason,
                retry_after_ms: 1000
            }
        );
        hub.cancel_turn("client-a", "main", &turn).unwrap();
    }
}

#[test]
fn no_provider_endpoint_or_credential_is_in_advisory_handoff() {
    let mut hub = fixture();
    let route = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    let token = permit(
        hub.acquire_permit("client-a", "main", &route.id, "r1", 0)
            .unwrap(),
    );
    assert_eq!(token.turn_id, "t1");
    assert_eq!(token.catalog_revision, route.catalog_revision);
    assert_eq!(token.logical_model_id, route.logical_model_id);
    assert_eq!(token.deployment_id, route.deployment_id);
    assert_eq!(token.provider_profile, "openai_compatible_chat");
    for json in [
        serde_json::to_value(route).unwrap(),
        serde_json::to_value(token).unwrap(),
    ] {
        assert_eq!(json["advisory_only"], true);
        assert!(json.get("endpoint").is_none());
        assert!(json.get("credential").is_none());
        assert!(json.get("provider_model").is_none());
    }
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.deployments[0].endpoint = "http://user:secret@localhost:1234/v1".into();
    assert!(matches!(
        hub.replace_catalog(catalog, 1),
        Err(HubError::InvalidCatalog { .. })
    ));
}

#[test]
fn missing_provider_model_requires_configuration_but_other_host_can_serve_same_model() {
    let mut hub = fixture();
    hub.observe_deployment("host-a", Availability::ModelMissing, 0)
        .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "t1", 0),
        Err(HubError::DeploymentModelMissing {
            deployment_id: "host-a".into()
        })
    );
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog
        .deployments
        .push(deployment("host-a-2", "model-a", "pool-b"));
    let revision = hub.replace_catalog(catalog, 0).unwrap();
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    hub.observe_deployment("host-a-2", Availability::Ready, 0)
        .unwrap();
    assert_eq!(
        lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap()).deployment_id,
        "host-a-2"
    );
    hub.cancel_turn("client-a", "main", "t1").unwrap();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog
        .deployments
        .retain(|deployment| deployment.logical_model_id != "model-a");
    let revision = hub.replace_catalog(catalog, 0).unwrap();
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "t2", 0),
        Err(HubError::DeploymentRequired)
    );
}

#[test]
fn protocol_tokens_are_canonical_but_display_labels_allow_unicode() {
    let mut hub = fixture();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models[0].label = "高性能モデル".into();
    let revision = hub.replace_catalog(catalog.clone(), 0).unwrap();
    for invalid_id in ["space id", "model:1", "日本語", ""] {
        let mut invalid = catalog.clone();
        invalid.models[0].id = invalid_id.into();
        assert!(invalid.validate().is_err());
    }
    for invalid_capability in ["tool use".to_owned(), "能力".to_owned(), "a".repeat(65)] {
        let mut invalid = catalog.clone();
        invalid.models[0]
            .capabilities
            .insert(invalid_capability.clone());
        assert!(invalid.validate().is_err());
        let mut selected = selection(revision);
        selected.required_capabilities.insert(invalid_capability);
        assert!(matches!(
            hub.save_selection("client-a", "main", selected),
            Err(HubError::InvalidSelection { .. })
        ));
    }
}

#[test]
fn restarting_preserves_catalog_identity_but_does_not_restore_runtime_authority() {
    let mut hub = fixture();
    let route = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    permit(
        hub.acquire_permit("client-a", "main", &route.id, "r1", 0)
            .unwrap(),
    );
    let persisted = serde_json::to_string(hub.catalog_store()).unwrap();
    let mut restarted = HubCore::new(serde_json::from_str(&persisted).unwrap()).unwrap();
    assert_eq!(restarted.catalog_store().hub_id, hub.catalog_store().hub_id);
    assert_eq!(
        restarted.catalog_store().revision,
        hub.catalog_store().revision
    );
    assert!(restarted.client_rows(1).is_empty());
    assert!(
        restarted
            .deployment_rows(1)
            .iter()
            .all(|row| row.availability == Availability::Unknown && row.active_permits == 0)
    );
    assert_eq!(
        restarted.acquire_permit("client-a", "main", &route.id, "r1", 1),
        Err(HubError::UnknownClient)
    );
    restarted
        .register_client("client-a", "Desktop", "192.0.2.1", 1)
        .unwrap();
    assert_eq!(
        restarted.begin_turn("client-a", "main", "t2", 1),
        Err(HubError::SelectionRequired)
    );
}

#[test]
fn unknown_persisted_fields_fail_decode_before_startup_can_rewrite_the_catalog() {
    let hub = fixture();
    let original = serde_json::to_value(hub.catalog_store()).unwrap();
    assert!(serde_json::from_value::<CatalogStore>(original.clone()).is_ok());
    for path in [
        "",
        "/catalog",
        "/catalog/models/0",
        "/catalog/deployments/0",
        "/catalog/capacity_pools/0",
        "/catalog/policy",
        "/changes/0",
    ] {
        let mut invalid = original.clone();
        // This future policy must never be discarded while retaining schema/review revision.
        invalid
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("require_signed_permits".into(), serde_json::json!(true));
        let error = serde_json::from_value::<CatalogStore>(invalid).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown field `require_signed_permits`"),
            "{path}: {error}"
        );
    }
}

#[test]
fn missing_persisted_schema_and_policy_fields_fail_decode() {
    let hub = fixture();
    let original = serde_json::to_value(hub.catalog_store()).unwrap();
    for (path, field) in [("", "schema_version"), ("/catalog/policy", "permit_ttl_ms")] {
        let mut invalid = original.clone();
        invalid
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            serde_json::from_value::<CatalogStore>(invalid).is_err(),
            "{path}/{field}"
        );
    }
}

#[test]
fn lease_expiry_and_removal_do_not_reassign_active_turn_silently() {
    let mut hub = fixture();
    let route = lease(hub.begin_turn("client-a", "main", "t1", 0).unwrap());
    assert_eq!(
        hub.begin_turn("client-a", "main", "t1", 5000),
        Err(HubError::LeaseExpired)
    );
    assert_eq!(
        hub.acquire_permit("client-a", "main", &route.id, "r1", 5000),
        Err(HubError::LeaseExpired)
    );
    hub.cancel_turn("client-a", "main", "t1").unwrap();
    let replacement = lease(hub.begin_turn("client-a", "main", "t2", 5000).unwrap());
    assert_ne!(replacement.id, route.id);
    let active = permit(
        hub.acquire_permit("client-a", "main", &replacement.id, "r1", 5000)
            .unwrap(),
    );
    hub.remove_client("client-a").unwrap();
    assert_eq!(hub.deployment_rows(5000)[0].active_permits, 0);
    assert_eq!(
        hub.release_permit("client-a", "main", &active.id),
        Err(HubError::UnknownClient)
    );
}

#[test]
fn active_turn_heartbeats_cross_lease_boundaries_without_reissuing_requests_or_affinity() {
    let mut hub = fixture();
    let first = lease(hub.begin_turn("client-a", "main", "long-turn", 0).unwrap());
    let ticket = permit(
        hub.acquire_permit("client-a", "main", &first.id, "first", 0)
            .unwrap(),
    );
    for now in [400, 4000, 8000, 12000, 16000] {
        hub.heartbeat_with_turns(
            "client-a",
            ClientActivity::Waiting,
            &[("main", "long-turn")],
            now,
        )
        .unwrap();
        hub.observe_deployment("host-a", Availability::Ready, now)
            .unwrap();
        let current = lease(
            hub.begin_turn("client-a", "main", "long-turn", now)
                .unwrap(),
        );
        assert_eq!(current.id, first.id);
        assert_eq!(current.deployment_id, first.deployment_id);
        assert_eq!(current.turns_remaining, first.turns_remaining);
        assert_eq!(current.expires_at_ms, now + 5000);
        if now == 400 {
            assert_eq!(
                permit(
                    hub.acquire_permit("client-a", "main", &first.id, "first", now)
                        .unwrap()
                ),
                ticket
            );
        }
    }
    assert_eq!(
        hub.acquire_permit("client-a", "main", &first.id, "first", 16000),
        Err(HubError::PermitExpired)
    );
    let next = permit(
        hub.acquire_permit("client-a", "main", &first.id, "second", 16000)
            .unwrap(),
    );
    assert_eq!(next.lease_id, first.id);
    assert_ne!(next.id, ticket.id);
    hub.finish_turn("client-a", "main", "long-turn").unwrap();
    let affinity = lease(
        hub.begin_turn("client-a", "main", "next-turn", 16001)
            .unwrap(),
    );
    assert_eq!(affinity.id, first.id);
    assert_eq!(affinity.turns_remaining, first.turns_remaining - 1);
}

#[test]
fn heartbeats_do_not_revive_expired_closed_wrong_context_or_other_client_turns() {
    let mut hub = fixture();
    for client in ["client-a", "client-b"] {
        hub.begin_turn(client, "main", "same-turn", 0).unwrap();
    }
    hub.heartbeat_with_turns(
        "client-a",
        ClientActivity::Running,
        &[("side_chat", "same-turn"), ("main", "wrong-turn")],
        4000,
    )
    .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "same-turn", 5000),
        Err(HubError::LeaseExpired)
    );
    hub.heartbeat_with_turns(
        "client-a",
        ClientActivity::Running,
        &[("main", "same-turn")],
        5000,
    )
    .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "same-turn", 5001),
        Err(HubError::LeaseExpired)
    );
    assert_eq!(
        hub.begin_turn("client-b", "main", "same-turn", 5001),
        Err(HubError::LeaseExpired)
    );
    hub.cancel_turn("client-a", "main", "same-turn").unwrap();
    hub.heartbeat_with_turns(
        "client-a",
        ClientActivity::Running,
        &[("main", "same-turn")],
        5002,
    )
    .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "same-turn", 5002),
        Err(HubError::TurnClosed)
    );
    let next = lease(
        hub.begin_turn("client-a", "main", "new-turn", 5002)
            .unwrap(),
    );
    hub.heartbeat_with_turns(
        "client-a",
        ClientActivity::Running,
        &[("main", "same-turn")],
        9000,
    )
    .unwrap();
    assert_eq!(
        lease(
            hub.begin_turn("client-a", "main", "new-turn", 9000)
                .unwrap()
        ),
        next
    );
    hub.remove_client("client-a").unwrap();
    assert_eq!(
        hub.heartbeat_with_turns(
            "client-a",
            ClientActivity::Running,
            &[("main", "new-turn")],
            9001
        ),
        Err(HubError::UnknownClient)
    );
    let mut restarted = HubCore::new(hub.catalog_store().clone()).unwrap();
    assert_eq!(
        restarted.heartbeat_with_turns(
            "client-b",
            ClientActivity::Running,
            &[("main", "same-turn")],
            9001
        ),
        Err(HubError::UnknownClient)
    );
}

#[test]
fn turn_heartbeat_does_not_bypass_review_or_extend_omitted_turns() {
    let mut hub = fixture();
    hub.begin_turn("client-a", "main", "turn", 0).unwrap();
    hub.heartbeat("client-a", ClientActivity::Running, 4000)
        .unwrap();
    assert_eq!(
        hub.begin_turn("client-a", "main", "turn", 5000),
        Err(HubError::LeaseExpired)
    );
    let mut hub = fixture();
    hub.begin_turn("client-a", "main", "turn", 0).unwrap();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models[0].label = "Changed".into();
    hub.replace_catalog(catalog, 100).unwrap();
    hub.heartbeat_with_turns(
        "client-a",
        ClientActivity::Running,
        &[("main", "turn")],
        4000,
    )
    .unwrap();
    assert!(matches!(
        hub.begin_turn("client-a", "main", "turn", 5001),
        Err(HubError::ReviewRequired { .. })
    ));
    assert!(
        hub.heartbeat_with_turns(
            "client-a",
            ClientActivity::Running,
            &[("main", "turn"), ("main", "turn")],
            5001
        )
        .is_err()
    );
}

#[test]
fn contexts_isolate_review_turn_retry_permit_and_cancellation_owners() {
    let mut hub = fixture();
    let revision = hub.catalog_store().revision;
    let mut side = selection(revision);
    side.allowed_model_ids = vec!["model-b".into()];
    side.preferred_model_id = Some("model-b".into());
    hub.save_selection("client-a", "side_chat", side.clone())
        .unwrap();
    let main_route = lease(hub.begin_turn("client-a", "main", "same-turn", 0).unwrap());
    let side_route = lease(
        hub.begin_turn("client-a", "side_chat", "same-turn", 0)
            .unwrap(),
    );
    let main_permit = permit(
        hub.acquire_permit("client-a", "main", &main_route.id, "same-request", 0)
            .unwrap(),
    );
    let side_permit = permit(
        hub.acquire_permit("client-a", "side_chat", &side_route.id, "same-request", 0)
            .unwrap(),
    );
    assert_eq!(main_route.context_id, "main");
    assert_eq!(side_route.context_id, "side_chat");
    assert_eq!(main_permit.context_id, "main");
    assert_eq!(side_permit.context_id, "side_chat");
    assert_ne!(main_permit.id, side_permit.id);
    assert_eq!(
        hub.acquire_permit("client-a", "main", &side_route.id, "other-request", 0),
        Err(HubError::WrongOwner)
    );
    assert_eq!(
        hub.release_permit("client-a", "main", &side_permit.id),
        Err(HubError::WrongOwner)
    );
    assert_eq!(
        hub.save_selection("client-a", "main", selection(revision)),
        Err(HubError::ActiveTurn)
    );
    hub.cancel_turn("client-a", "side_chat", "same-turn")
        .unwrap();
    hub.save_selection("client-a", "side_chat", side).unwrap();
    assert_eq!(
        permit(
            hub.acquire_permit("client-a", "main", &main_route.id, "same-request", 0)
                .unwrap()
        ),
        main_permit
    );
    assert_eq!(
        hub.begin_turn("client-a", "side_chat", "same-turn", 0),
        Err(HubError::TurnClosed)
    );
    assert_eq!(
        hub.deployment_rows(0)
            .iter()
            .map(|row| row.active_permits)
            .sum::<u32>(),
        1
    );
    hub.remove_client("client-a").unwrap();
    assert_eq!(
        hub.deployment_rows(0)
            .iter()
            .map(|row| row.active_permits)
            .sum::<u32>(),
        0
    );
}

#[test]
fn reviewing_main_does_not_review_side_after_catalog_update() {
    let mut hub = fixture();
    hub.save_selection(
        "client-a",
        "side_chat",
        selection(hub.catalog_store().revision),
    )
    .unwrap();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.models[0].label = "Updated model".into();
    let revision = hub.replace_catalog(catalog, 1).unwrap();
    hub.save_selection("client-a", "main", selection(revision))
        .unwrap();
    let clients = hub.client_rows(1);
    let client = clients
        .iter()
        .find(|client| client.id == "client-a")
        .unwrap();
    assert_eq!(client.contexts.len(), 2);
    assert!(
        !client
            .contexts
            .iter()
            .find(|context| context.context_id == "main")
            .unwrap()
            .review_required
    );
    assert!(
        client
            .contexts
            .iter()
            .find(|context| context.context_id == "side_chat")
            .unwrap()
            .review_required
    );
    assert!(matches!(
        hub.begin_turn("client-a", "side_chat", "t1", 1),
        Err(HubError::ReviewRequired { .. })
    ));
    assert!(matches!(
        hub.begin_turn("client-a", "main", "t1", 1),
        Ok(RouteDecision::Assigned { .. })
    ));
}

#[test]
fn context_registry_bound_rejects_new_context_without_losing_existing_reviews() {
    let mut hub = fixture();
    let selected = selection(hub.catalog_store().revision);
    for index in 1..16 {
        hub.save_selection("client-a", &format!("context-{index}"), selected.clone())
            .unwrap();
    }
    assert_eq!(
        hub.save_selection("client-a", "overflow", selected.clone()),
        Err(HubError::RegistryFull)
    );
    assert!(hub.save_selection("client-a", "main", selected).is_ok());
    let clients = hub.client_rows(0);
    let client = clients
        .iter()
        .find(|client| client.id == "client-a")
        .unwrap();
    assert_eq!(client.contexts.len(), 16);
    assert!(
        client
            .contexts
            .iter()
            .all(|context| !context.review_required)
    );
}
