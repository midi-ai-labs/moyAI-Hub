use super::*;
use crate::identity::{self, IdentityError};
use crate::shared_work::{ManagementCommand, WorkError};

pub(super) fn owns_command(command: &str) -> bool {
    matches!(
        command,
        "hub_shared_snapshot"
            | "hub_shared_command"
            | "hub_shared_retention_preview"
            | "hub_shared_maintenance"
            | "hub_join_project"
            | "hub_network_settings"
            | "hub_network_invite"
            | "hub_network_approve"
            | "hub_network_admission"
            | "hub_network_group"
            | "hub_network_device"
            | "hub_network_rule"
            | "hub_network_delete_device"
            | "hub_network_delete_rule"
            | "hub_history_request"
    )
}

pub(super) async fn command(
    admin: Arc<Admin>,
    input: Command,
    token: String,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    let operation = tokio::task::spawn_blocking(move || -> crate::shared_work::Result<Value> {
        let _permit = permit;
        let mut store = admin
            .shared_work
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        // Reauthenticate after waiting for the owner: logout/session replacement cannot
        // authorize a queued management mutation with a previously captured Principal.
        let mut network = admin
            .network
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
        let actor = identity::web_access::authenticate(
            store.identity_connection(),
            &network,
            &token,
            crate::service::now_ms(),
        )?;
        store.expire_start_deadlines(crate::service::now_ms())?;
        if input.command.starts_with("hub_network_") || input.command == "hub_history_request" {
            return crate::admin::dispatch_network_locked(
                &store,
                &mut network,
                &input.command,
                &input.args,
            )
            .map_err(WorkError::Management);
        }
        if input.command == "hub_join_project" {
            return approve_and_assign(&mut network, &mut store, &actor, input.args);
        }
        if input.command == "hub_shared_snapshot" {
            if !input.args.as_object().is_some_and(|v| v.is_empty()) {
                return Err(WorkError::Invalid);
            }
            return store.management_snapshot(&actor);
        }
        if input.command == "hub_shared_retention_preview" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Preview {
                #[serde(rename = "beforeMs")]
                before: u64,
            }
            let preview: Preview =
                serde_json::from_value(input.args).map_err(|_| WorkError::Invalid)?;
            return store.preview_retention(&actor, preview.before);
        }
        if input.command == "hub_shared_maintenance" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Page {
                before: Option<String>,
                limit: Option<u32>,
            }
            let page: Page = serde_json::from_value(input.args).map_err(|_| WorkError::Invalid)?;
            return store.management_maintenance(
                &actor,
                page.before.as_deref(),
                page.limit.unwrap_or(128),
            );
        }
        let request: ManagementCommand =
            serde_json::from_value(input.args).map_err(|_| WorkError::Invalid)?;
        if let Some(devices) = crate::shared_work::api::management::project_device_targets(&request)
        {
            for device in devices {
                network
                    .self_projection(&device, crate::service::now_ms())
                    .map_err(|_| WorkError::Forbidden)?;
            }
        }
        if let Some(runner) = crate::shared_work::api::management::environment_target(&request) {
            network
                .admission_revision(&runner, crate::service::now_ms())
                .map_err(|_| WorkError::Forbidden)?;
        }
        let labels = crate::shared_work::api::management::project_device_targets(&request)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| {
                network
                    .device_label(&id)
                    .map(|label| (id.clone(), label.to_owned()))
            })
            .collect();
        store.manage_with_device_labels(&actor, &request, crate::service::now_ms(), &labels)
    });
    match tokio::time::timeout(Duration::from_secs(30), operation).await {
        Ok(Ok(Ok(value))) => Json(json!({"result":value})).into_response(),
        Ok(Ok(Err(error))) => crate::shared_work::api::error(error).into_response(),
        _ => crate::shared_work::api::error(IdentityError::Unavailable.into()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JoinProject {
    request_id: String,
    key_sha256: String,
    label: String,
    expected_network_revision: String,
    command_id: String,
    expected_shared_revision: String,
    project: Option<JoinPurpose>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinPurpose {
    id: String,
    can_control: bool,
    can_execute: bool,
}

fn approve_and_assign(
    network: &mut crate::network::DeviceNetwork,
    store: &mut crate::shared_work::SharedWork,
    actor: &identity::Principal,
    args: Value,
) -> crate::shared_work::Result<Value> {
    identity::require_admin(store.identity_connection(), actor)?;
    let input: JoinProject = serde_json::from_value(args).map_err(|_| WorkError::Invalid)?;
    let snapshot = store.management_snapshot(actor)?;
    let shared_current =
        snapshot["revision"].as_str() == Some(input.expected_shared_revision.as_str());
    let approved = network
        .approved_join_device(&input.request_id, &input.key_sha256)
        .map_err(|_| WorkError::Conflict)?;
    if approved.is_none() {
        if let Some(project) = &input.project {
            if (!project.can_control && !project.can_execute)
                || !snapshot["projects"].as_array().is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row["id"] == project.id
                            && row["archived"] == false
                            && row["legacy_devices_unrestricted"] == false
                    })
                })
            {
                return Err(WorkError::Invalid);
            }
        }
    }
    let device = if let Some(device) = approved {
        device
    } else {
        network
            .approve_join(
                &input.expected_network_revision,
                &input.request_id,
                input.label,
                Default::default(),
                crate::service::now_ms(),
            )
            .map_err(|_| WorkError::Conflict)?;
        network
            .approved_join_device(&input.request_id, &input.key_sha256)
            .map_err(|_| WorkError::Conflict)?
            .ok_or(WorkError::Conflict)?
    };
    network
        .self_projection(&device, crate::service::now_ms())
        .map_err(|_| WorkError::Forbidden)?;
    let label = network.device_label(&device).unwrap_or(&device);
    if let Err(error) = identity::device::enroll(
        store.identity_connection(),
        &device,
        label,
        crate::service::now_ms(),
    ) {
        return Ok(
            json!({"device_id":device,"project_saved":false,"identity_saved":false,"error":error.to_string(),"snapshot":store.management_snapshot(actor)?}),
        );
    }
    // Network JSON and shared SQLite are different owners. A project error
    // must return the durable approved device, never pretend approval rolled back.
    if let Some(project) = input.project {
        let command = ManagementCommand {
            request_id: input.command_id,
            expected_revision: if shared_current {
                store.management_snapshot(actor)?["revision"]
                    .as_str()
                    .ok_or(WorkError::Conflict)?
                    .to_owned()
            } else {
                input.expected_shared_revision
            },
            request: crate::shared_work::ManagementRequest::AssignProjectDevice {
                project_id: project.id,
                device_id: device.clone(),
                can_control: project.can_control,
                can_execute: project.can_execute,
            },
        };
        match store.manage(actor, &command, crate::service::now_ms()) {
            Ok(snapshot) => Ok(
                json!({"device_id":device,"project_saved":true,"error":null,"snapshot":snapshot}),
            ),
            Err(error) => Ok(
                json!({"device_id":device,"project_saved":false,"error":error.to_string(),"snapshot":store.management_snapshot(actor)?}),
            ),
        }
    } else {
        Ok(
            json!({"device_id":device,"project_saved":true,"identity_saved":true,"error":null,"snapshot":store.management_snapshot(actor)?}),
        )
    }
}
