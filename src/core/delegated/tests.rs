use super::*;
use crate::catalog::{CapacityPool, CatalogPolicy, LogicalModel};

fn fixture() -> (HubCore, Selection) {
    let mut store = CatalogStore::empty("delegated-fixture");
    store
        .replace_catalog(
            Catalog {
                team_default_model_id: None,
                models: vec![LogicalModel {
                    system_prompt: String::new(),
                    id: "model".into(),
                    label: "Model".into(),
                    capabilities: BTreeSet::from(["tools".into()]),
                }],
                deployments: vec![Deployment {
                    id: "deployment".into(),
                    logical_model_id: "model".into(),
                    capacity_pool_id: "pool".into(),
                    label: "Provider".into(),
                    endpoint: "http://127.0.0.1:1234".into(),
                    provider_model: "model".into(),
                    provider_profile: "openai_compatible_chat".into(),
                    upstream_credential_id: None,
                    enabled: true,
                }],
                capacity_pools: vec![CapacityPool {
                    id: "pool".into(),
                    label: "Pool".into(),
                    max_concurrent: 1,
                }],
                policy: CatalogPolicy {
                    global_max_concurrent: 1,
                    permit_ttl_ms: 1000,
                    heartbeat_stale_ms: 10_000,
                    lease_ttl_ms: 20_000,
                },
            },
            0,
        )
        .unwrap();
    let selection = Selection {
        allowed_model_ids: vec!["model".into()],
        preferred_model_id: Some("model".into()),
        required_capabilities: BTreeSet::from(["tools".into()]),
        wait_policy: WaitPolicy::WaitForPreferred,
        affinity_turns: 3,
        reviewed_revision: store.revision,
    };
    let mut hub = HubCore::new(store).unwrap();
    hub.register_client("client", "Client", "127.0.0.1", 0)
        .unwrap();
    hub.observe_present_deployment("deployment", Availability::Unknown, 0)
        .unwrap();
    for context in ["main", "side_chat"] {
        hub.save_selection("client", context, selection.clone())
            .unwrap();
    }
    (hub, selection)
}
fn id(value: u128) -> String {
    ulid::Ulid::from(value).to_string()
}
fn child(hub: &mut HubCore, selection: &Selection, value: u128) -> (String, String) {
    let execution = id(value);
    let context = hub
        .ensure_delegated_context("client", "main", &execution, selection.clone(), 0)
        .unwrap();
    (context, execution)
}
fn permit(value: PermitDecision) -> ExecutionPermit {
    match value {
        PermitDecision::Granted { permit } => permit,
        other => panic!("{other:?}"),
    }
}

#[test]
fn delegated_scopes_share_fifo_capacity_without_consuming_parent_affinity_or_closing_siblings() {
    let (mut hub, selection) = fixture();
    let parent = permit(
        hub.prepare_request(
            "client",
            "main",
            "parent",
            "request",
            RequestPurpose::UserTurn,
            0,
        )
        .unwrap(),
    );
    hub.request_admitted(&parent);
    let (first, first_id) = child(&mut hub, &selection, 1);
    let (second, second_id) = child(&mut hub, &selection, 2);
    for (context, execution) in [(&first, &first_id), (&second, &second_id)] {
        assert!(matches!(
            hub.prepare_request(
                "client",
                context,
                execution,
                "request",
                RequestPurpose::Delegated,
                0
            )
            .unwrap(),
            PermitDecision::Waiting { .. }
        ));
    }
    hub.finish_turn("client", "main", "parent").unwrap();
    assert!(matches!(
        hub.prepare_request(
            "client",
            &second,
            &second_id,
            "request",
            RequestPurpose::Delegated,
            1
        )
        .unwrap(),
        PermitDecision::Waiting { .. }
    ));
    let accepted = permit(
        hub.prepare_request(
            "client",
            &first,
            &first_id,
            "request",
            RequestPurpose::Delegated,
            1,
        )
        .unwrap(),
    );
    assert_eq!(
        hub.clients["client"].contexts[&first]
            .lease
            .as_ref()
            .unwrap()
            .turns_remaining,
        3
    );
    hub.request_admitted(&accepted);
    hub.close_delegated_context("client", "main", &first_id, true)
        .unwrap();
    assert!(hub.validate_execution_permit(&accepted.id, 1).is_err());
    let next = permit(
        hub.prepare_request(
            "client",
            &second,
            &second_id,
            "request",
            RequestPurpose::Delegated,
            1,
        )
        .unwrap(),
    );
    assert_eq!(next.context_id, second);
    hub.close_delegated_context("client", "main", &second_id, false)
        .unwrap();
    let RouteDecision::Assigned { lease } =
        hub.begin_turn("client", "main", "next-parent", 1).unwrap()
    else {
        panic!("parent route")
    };
    assert_eq!(
        lease.turns_remaining, 1,
        "only two parent turns consumed affinity"
    );
    assert_eq!(
        hub.client_rows(1)[0].contexts.len(),
        2,
        "child scopes do not become review rows"
    );
}

#[test]
fn delegated_selection_is_captured_once_and_rejects_context_purpose_or_revision_rebinding() {
    let (mut hub, selection) = fixture();
    let (context, execution) = child(&mut hub, &selection, 1);
    assert!(
        hub.begin_request("client", &context, &execution, RequestPurpose::UserTurn, 0)
            .is_err()
    );
    assert!(
        hub.begin_request("client", "main", &execution, RequestPurpose::Delegated, 0)
            .is_err()
    );
    assert!(
        hub.save_selection("client", &context, selection.clone())
            .is_err()
    );
    assert!(
        hub.ensure_delegated_context("client", "side_chat", &execution, selection.clone(), 0)
            .is_err()
    );
    let mut changed = selection.clone();
    changed.affinity_turns = 2;
    hub.save_selection("client", "main", changed.clone())
        .unwrap();
    assert!(
        hub.ensure_delegated_context("client", "main", &execution, selection.clone(), 0)
            .is_ok()
    );
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &execution, changed.clone(), 0),
        Err(HubError::ReviewRequired { .. })
    ));
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &id(2), selection.clone(), 0),
        Err(HubError::ReviewRequired { .. })
    ));
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.policy.global_max_concurrent = 2;
    hub.replace_catalog(catalog, 1).unwrap();
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &execution, selection, 1),
        Err(HubError::ReviewRequired { .. })
    ));
    hub.close_delegated_context("client", "main", &execution, false)
        .unwrap();
}

#[test]
fn delegated_heartbeat_renews_only_existing_child_and_disconnect_retires_its_queue() {
    let (mut hub, selection) = fixture();
    let (context, execution) = child(&mut hub, &selection, 1);
    let granted = permit(
        hub.prepare_request(
            "client",
            &context,
            &execution,
            "r",
            RequestPurpose::Delegated,
            0,
        )
        .unwrap(),
    );
    hub.heartbeat_with_turns(
        "client",
        ClientActivity::Running,
        &[(&context, &execution)],
        5_000,
    )
    .unwrap();
    assert_eq!(
        hub.clients["client"].contexts[&context]
            .active_turn
            .as_ref()
            .unwrap()
            .lease
            .expires_at_ms,
        25_000
    );
    assert_eq!(
        granted.expires_at_ms, 1_000,
        "heartbeat cannot extend permit expiry"
    );
    hub.close_delegated_context("client", "main", &execution, false)
        .unwrap();
    hub.heartbeat_with_turns(
        "client",
        ClientActivity::Running,
        &[(&context, &execution)],
        5_001,
    )
    .unwrap();
    assert!(!hub.clients["client"].contexts.contains_key(&context));
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &execution, selection.clone(), 5_001),
        Err(HubError::TurnClosed)
    ));
    let (context, execution) = child(&mut hub, &selection, 2);
    hub.prepare_request(
        "client",
        &context,
        &execution,
        "r",
        RequestPurpose::Delegated,
        5_001,
    )
    .unwrap();
    hub.remove_client("client").unwrap();
    assert!(hub.request_queue.is_empty());
    assert!(hub.permits.is_empty());
}

#[test]
fn delegated_close_overtakes_prepare_and_both_active_and_retired_identity_sets_are_bounded() {
    let (mut hub, selection) = fixture();
    hub.close_delegated_context("client", "main", &id(1), true)
        .unwrap();
    hub.close_delegated_context("client", "main", &id(1), true)
        .unwrap();
    assert!(
        hub.close_delegated_context("client", "side_chat", &id(1), true)
            .is_err()
    );
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &id(1), selection.clone(), 0),
        Err(HubError::TurnClosed)
    ));
    for value in 2..=15 {
        child(&mut hub, &selection, value);
    }
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &id(16), selection.clone(), 0),
        Err(HubError::RegistryFull)
    ));
    hub.close_delegated_context("client", "main", &id(2), false)
        .unwrap();
    child(&mut hub, &selection, 16);
    let (mut full, selection) = fixture();
    for value in 1..=MAX_DELEGATIONS_PER_CLIENT {
        full.close_delegated_context("client", "main", &id(value as u128), true)
            .unwrap();
    }
    assert!(matches!(
        full.ensure_delegated_context("client", "main", &id(1), selection.clone(), 0),
        Err(HubError::TurnClosed)
    ));
    assert!(matches!(
        full.ensure_delegated_context("client", "main", &id(5000), selection, 0),
        Err(HubError::RegistryFull)
    ));
}

#[test]
fn delegated_presence_expiry_rejects_claim_and_a_late_explicit_heartbeat_cannot_revive_it() {
    let (mut hub, selection) = fixture();
    hub.store.catalog.policy.permit_ttl_ms = 20_000;
    let (context, execution) = child(&mut hub, &selection, 1);
    let admitted = permit(
        hub.prepare_request(
            "client",
            &context,
            &execution,
            "request",
            RequestPurpose::Delegated,
            0,
        )
        .unwrap(),
    );
    hub.heartbeat("client", ClientActivity::Idle, 9000).unwrap();
    assert!(matches!(
        hub.validate_execution_permit(&admitted.id, 10_000),
        Err(HubError::TurnClosed)
    ));
    hub.heartbeat_with_turns(
        "client",
        ClientActivity::Running,
        &[(&context, &execution)],
        10_000,
    )
    .unwrap();
    assert!(!hub.clients["client"].contexts.contains_key(&context));
    assert!(matches!(
        hub.ensure_delegated_context("client", "main", &execution, selection, 10_000),
        Err(HubError::TurnClosed)
    ));
    assert!(hub.permits.is_empty());
    assert!(hub.request_queue.is_empty());
}
