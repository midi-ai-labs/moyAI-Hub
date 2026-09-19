use super::*;
use crate::identity::{self, IdentityError, SessionAudience};
use crate::shared_work::{ManagementCommand, WorkError};

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
        let actor = identity::authenticate(
            store.identity_connection(),
            &token,
            SessionAudience::WebAdmin,
            None,
            crate::service::now_ms(),
        )?;
        store.expire_start_deadlines(crate::service::now_ms())?;
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
        let network = admin
            .network
            .lock()
            .map_err(|_| IdentityError::Unavailable)?;
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
        store.manage(&actor, &request, crate::service::now_ms())
    });
    match tokio::time::timeout(Duration::from_secs(30), operation).await {
        Ok(Ok(Ok(value))) => Json(json!({"result":value})).into_response(),
        Ok(Ok(Err(error))) => crate::shared_work::api::error(error).into_response(),
        _ => crate::shared_work::api::error(IdentityError::Unavailable.into()).into_response(),
    }
}
