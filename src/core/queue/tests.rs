use super::*;
use crate::catalog::{CapacityPool, CatalogPolicy, LogicalModel};

fn fixture() -> HubCore {
    let mut store = CatalogStore::empty("queue-fixture");
    let catalog = Catalog {
        models: ["a", "b"]
            .into_iter()
            .map(|id| LogicalModel {
                id: id.into(),
                label: id.into(),
                capabilities: BTreeSet::from(["tools".into()]),
            })
            .collect(),
        deployments: ["a", "b"]
            .into_iter()
            .map(|id| Deployment {
                id: id.into(),
                logical_model_id: id.into(),
                capacity_pool_id: id.into(),
                label: id.into(),
                endpoint: "http://127.0.0.1:1234/v1".into(),
                provider_model: id.into(),
                provider_profile: "openai_compatible_chat".into(),
                enabled: true,
            })
            .collect(),
        capacity_pools: ["a", "b"]
            .into_iter()
            .map(|id| CapacityPool {
                id: id.into(),
                label: id.into(),
                max_concurrent: 1,
            })
            .collect(),
        policy: CatalogPolicy {
            global_max_concurrent: 2,
            permit_ttl_ms: 1000,
            heartbeat_stale_ms: 10_000,
            lease_ttl_ms: 20_000,
        },
    };
    store.replace_catalog(catalog, 0).unwrap();
    let mut hub = HubCore::new(store).unwrap();
    for model in ["a", "b"] {
        hub.observe_deployment(model, Availability::Ready, 0)
            .unwrap();
    }
    for client in ["running", "older", "later"] {
        hub.register_client(client, client, "127.0.0.1", 0).unwrap();
        select(&mut hub, client, "a");
    }
    hub
}
fn select(hub: &mut HubCore, client: &str, model: &str) {
    hub.save_selection(
        client,
        "main",
        Selection {
            allowed_model_ids: vec![model.into()],
            preferred_model_id: Some(model.into()),
            required_capabilities: BTreeSet::from(["tools".into()]),
            wait_policy: WaitPolicy::WaitForPreferred,
            affinity_turns: 3,
            reviewed_revision: hub.catalog_store().revision,
        },
    )
    .unwrap();
}
fn prepare(hub: &mut HubCore, client: &str, request: &str, now: u64) -> PermitDecision {
    hub.prepare_request(
        client,
        "main",
        "turn",
        request,
        RequestPurpose::UserTurn,
        now,
    )
    .unwrap()
}
fn granted(decision: PermitDecision) -> ExecutionPermit {
    match decision {
        PermitDecision::Granted { permit } => permit,
        other => panic!("expected permit: {other:?}"),
    }
}
fn admit(hub: &mut HubCore, client: &str, request: &str, now: u64) -> ExecutionPermit {
    let permit = granted(prepare(hub, client, request, now));
    hub.request_admitted(&permit);
    permit
}
fn waiting(decision: PermitDecision) {
    assert!(matches!(decision, PermitDecision::Waiting { .. }));
}

#[test]
fn request_fifo_prevents_the_previous_poll_order_overtake() {
    let mut baseline = fixture();
    let running = admit(&mut baseline, "running", "r", 0);
    waiting(prepare(&mut baseline, "older", "r", 1));
    baseline.settle_execution_permit(&running.id);
    // The former network implementation called these two primitives directly;
    // it allowed a later polling caller to take the newly released slot.
    let RouteDecision::Assigned { lease } =
        baseline.begin_turn("later", "main", "turn", 2).unwrap()
    else {
        panic!("route");
    };
    assert!(matches!(
        baseline
            .acquire_permit("later", "main", &lease.id, "r", 2)
            .unwrap(),
        PermitDecision::Granted { .. }
    ));

    let mut hub = fixture();
    let running = admit(&mut hub, "running", "r", 0);
    waiting(prepare(&mut hub, "older", "r", 1));
    waiting(prepare(&mut hub, "later", "r", 2));
    hub.settle_execution_permit(&running.id);
    waiting(prepare(&mut hub, "later", "r", 3));
    let first = admit(&mut hub, "older", "r", 4);
    waiting(prepare(&mut hub, "later", "r", 5));
    hub.settle_execution_permit(&first.id);
    admit(&mut hub, "later", "r", 6);
}

#[test]
fn companion_declines_and_retry_acknowledgements_preserve_exact_ticket_order() {
    let mut hub = fixture();
    let first = granted(prepare(&mut hub, "older", "r", 0));
    waiting(prepare(&mut hub, "later", "r", 1));
    let replay = granted(prepare(&mut hub, "older", "r", 2));
    assert_eq!(replay.id, first.id);
    hub.withdraw_declined_permit(&first);
    waiting(prepare(&mut hub, "later", "r", 3));
    let next = granted(prepare(&mut hub, "older", "r", 4));
    assert_ne!(next.id, first.id);
    hub.withdraw_declined_permit(&next);
    waiting(prepare(&mut hub, "later", "r", 5));
    let admitted = admit(&mut hub, "older", "r", 6);
    hub.settle_execution_permit(&admitted.id);
    admit(&mut hub, "later", "r", 7);
}

#[test]
fn unrelated_pool_keeps_running_and_global_capacity_still_orders_competitors() {
    let mut hub = fixture();
    let running = admit(&mut hub, "running", "r", 0);
    waiting(prepare(&mut hub, "older", "r", 1));
    select(&mut hub, "later", "b");
    let other = admit(&mut hub, "later", "r", 2);
    assert_eq!(other.deployment_id, "b");
    hub.settle_execution_permit(&running.id);
    hub.settle_execution_permit(&other.id);
    hub.finish_turn("later", "main", "turn").unwrap();
    // Even a runnable older pool does not block a distinct free pool when both fit.
    let independent = granted(
        hub.prepare_request(
            "later",
            "main",
            "second",
            "r",
            RequestPurpose::Preparation,
            3,
        )
        .unwrap(),
    );
    assert_eq!(independent.deployment_id, "b");
    admit(&mut hub, "older", "r", 4);

    let mut hub = fixture();
    let mut catalog = hub.catalog_store().catalog.clone();
    catalog.policy.global_max_concurrent = 1;
    hub.replace_catalog(catalog, 0).unwrap();
    for client in ["running", "older"] {
        select(&mut hub, client, "a");
    }
    select(&mut hub, "later", "b");
    let running = admit(&mut hub, "running", "r", 0);
    waiting(prepare(&mut hub, "older", "r", 1));
    waiting(prepare(&mut hub, "later", "r", 2));
    hub.settle_execution_permit(&running.id);
    assert!(matches!(
        prepare(&mut hub, "later", "r", 3),
        PermitDecision::Waiting {
            reason: WaitReason::GlobalCapacity,
            ..
        }
    ));
    admit(&mut hub, "older", "r", 4);
}

#[test]
fn cancelling_disconnect_and_catalog_review_retire_queue_positions() {
    for action in ["cancel", "disconnect", "revision"] {
        let mut hub = fixture();
        let running = admit(&mut hub, "running", "r", 0);
        waiting(prepare(&mut hub, "older", "r", 1));
        waiting(prepare(&mut hub, "later", "r", 2));
        match action {
            "cancel" => hub.cancel_turn("older", "main", "turn").unwrap(),
            "disconnect" => hub.remove_client("older").unwrap(),
            "revision" => {
                let mut catalog = hub.catalog_store().catalog.clone();
                catalog.policy.global_max_concurrent = 3;
                hub.replace_catalog(catalog, 3).unwrap();
                assert!(hub.request_queue.is_empty());
                assert!(matches!(
                    hub.prepare_request("older", "main", "turn", "r", RequestPurpose::UserTurn, 4),
                    Err(HubError::ReviewRequired { .. })
                ));
                continue;
            }
            _ => unreachable!(),
        }
        hub.settle_execution_permit(&running.id);
        admit(&mut hub, "later", "r", 3);
    }
}

#[test]
fn an_abandoned_request_does_not_keep_priority_from_desktop_heartbeats_alone() {
    let mut hub = fixture();
    let running = admit(&mut hub, "running", "r", 0);
    waiting(prepare(&mut hub, "older", "r", 1));
    waiting(prepare(&mut hub, "later", "r", 2));
    hub.settle_execution_permit(&running.id);
    for client in ["older", "later"] {
        hub.heartbeat(client, ClientActivity::Waiting, 9000)
            .unwrap();
    }
    hub.observe_deployment("a", Availability::Ready, 9000)
        .unwrap();
    admit(&mut hub, "later", "r", 10_001);
}
