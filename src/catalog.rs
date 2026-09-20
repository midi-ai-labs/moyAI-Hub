use crate::protocol::{Availability, HubError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SCHEMA_VERSION: u32 = 2;
pub const MAX_CATALOG_ENTRIES: usize = 1024;
pub const MAX_LOGICAL_MODELS: usize = 128;
const MAX_CHANGES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogicalModel {
    pub id: String,
    pub label: String,
    pub capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub id: String,
    pub logical_model_id: String,
    pub capacity_pool_id: String,
    pub label: String,
    /// Local administrator data. Public client catalogs must omit provider addresses.
    pub endpoint: String,
    pub provider_model: String,
    pub provider_profile: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapacityPool {
    pub id: String,
    pub label: String,
    pub max_concurrent: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogPolicy {
    pub global_max_concurrent: u32,
    pub permit_ttl_ms: u64,
    pub heartbeat_stale_ms: u64,
    pub lease_ttl_ms: u64,
}

impl Default for CatalogPolicy {
    fn default() -> Self {
        Self {
            global_max_concurrent: 8,
            permit_ttl_ms: 60_000,
            heartbeat_stale_ms: 30_000,
            lease_ttl_ms: 900_000,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Explicit initial choice for new model clients; existing client choices are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_default_model_id: Option<String>,
    pub models: Vec<LogicalModel>,
    pub deployments: Vec<Deployment>,
    pub capacity_pools: Vec<CapacityPool>,
    pub policy: CatalogPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogChange {
    pub revision: u64,
    pub changed_at_ms: u64,
    pub added_model_ids: Vec<String>,
    pub removed_model_ids: Vec<String>,
    pub changed_model_ids: Vec<String>,
    pub deployments_changed: bool,
    pub policy_changed: bool,
    pub software_changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogStore {
    pub schema_version: u32,
    pub hub_id: String,
    pub software_version: String,
    pub revision: u64,
    pub catalog: Catalog,
    /// Bounded recent history. A client older than this window must review the full catalog.
    pub changes: Vec<CatalogChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeploymentRow {
    pub deployment: Deployment,
    pub availability: Availability,
    /// A fresh successful provider listing contained this model. This is not load readiness.
    #[serde(default)]
    pub model_present: bool,
    pub last_observed_ms: Option<u64>,
    pub active_permits: u32,
}

impl CatalogStore {
    pub fn empty(software_version: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            hub_id: ulid::Ulid::new().to_string(),
            software_version: software_version.into(),
            revision: 1,
            catalog: Catalog::default(),
            changes: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), HubError> {
        let legacy = self.schema_version == 1 && self.catalog.team_default_model_id.is_none();
        if (!legacy && self.schema_version != SCHEMA_VERSION) || self.revision == 0 {
            return Err(invalid("unsupported schema or invalid revision"));
        }
        valid_text(&self.software_version, 128)?;
        if !valid_token(&self.hub_id, 128) {
            return Err(invalid("invalid Hub identity"));
        }
        self.catalog.validate()?;
        if self.changes.len() > MAX_CHANGES {
            return Err(invalid("revision history exceeds limit"));
        }
        let mut previous = 0;
        for change in &self.changes {
            if change.revision <= previous || change.revision > self.revision {
                return Err(invalid("invalid revision history"));
            }
            previous = change.revision;
        }
        Ok(())
    }

    /// Called once on load with the actual application version, then persisted before serving.
    pub fn reconcile_software(
        &mut self,
        software_version: &str,
        now_ms: u64,
    ) -> Result<bool, HubError> {
        self.validate()?;
        valid_text(software_version, 128)?;
        if self.software_version == software_version && self.schema_version == SCHEMA_VERSION {
            return Ok(false);
        }
        let revision = self.next_revision()?;
        self.schema_version = SCHEMA_VERSION;
        self.software_version = software_version.to_owned();
        self.record(CatalogChange {
            revision,
            changed_at_ms: now_ms,
            added_model_ids: vec![],
            removed_model_ids: vec![],
            changed_model_ids: vec![],
            deployments_changed: false,
            policy_changed: false,
            software_changed: true,
        });
        Ok(true)
    }

    pub fn replace_catalog(&mut self, catalog: Catalog, now_ms: u64) -> Result<u64, HubError> {
        catalog.validate()?;
        if self.catalog == catalog {
            return Ok(self.revision);
        }
        let revision = self.next_revision()?;
        let before = &self.catalog;
        let change = CatalogChange {
            revision,
            changed_at_ms: now_ms,
            added_model_ids: catalog
                .models
                .iter()
                .filter(|model| !before.models.iter().any(|old| old.id == model.id))
                .map(|model| model.id.clone())
                .collect(),
            removed_model_ids: before
                .models
                .iter()
                .filter(|model| !catalog.models.iter().any(|new| new.id == model.id))
                .map(|model| model.id.clone())
                .collect(),
            changed_model_ids: catalog
                .models
                .iter()
                .filter(|model| {
                    before
                        .models
                        .iter()
                        .any(|old| old.id == model.id && old != *model)
                })
                .map(|model| model.id.clone())
                .collect(),
            deployments_changed: before.deployments != catalog.deployments,
            policy_changed: before.policy != catalog.policy
                || before.capacity_pools != catalog.capacity_pools
                || before.team_default_model_id != catalog.team_default_model_id,
            software_changed: false,
        };
        self.catalog = catalog;
        self.schema_version = SCHEMA_VERSION;
        self.record(change);
        Ok(revision)
    }

    fn next_revision(&self) -> Result<u64, HubError> {
        self.revision
            .checked_add(1)
            .ok_or(HubError::RevisionExhausted)
    }

    fn record(&mut self, change: CatalogChange) {
        self.revision = change.revision;
        self.changes.push(change);
        if self.changes.len() > MAX_CHANGES {
            self.changes.remove(0);
        }
    }
}

impl Catalog {
    pub fn validate(&self) -> Result<(), HubError> {
        if let Some(id) = &self.team_default_model_id {
            if !self
                .models
                .iter()
                .any(|model| &model.id == id && model.capabilities.contains("tools"))
            {
                return Err(invalid(
                    "standard model must be an existing tools-capable model",
                ));
            }
        }
        if self.models.len() > MAX_LOGICAL_MODELS {
            return Err(invalid("logical model catalog exceeds 128 entries"));
        }
        let model_ids = unique_ids(self.models.iter().map(|value| value.id.as_str()))?;
        let pool_ids = unique_ids(self.capacity_pools.iter().map(|value| value.id.as_str()))?;
        unique_ids(self.deployments.iter().map(|value| value.id.as_str()))?;
        for model in &self.models {
            valid_text(&model.label, 256)?;
            if model.capabilities.len() > 32 {
                return Err(invalid("too many capabilities"));
            }
            for capability in &model.capabilities {
                if !valid_token(capability, 64) {
                    return Err(invalid("invalid capability token"));
                }
            }
        }
        for pool in &self.capacity_pools {
            valid_text(&pool.label, 256)?;
            if pool.max_concurrent == 0 || pool.max_concurrent > 1024 {
                return Err(invalid("capacity must be between 1 and 1024"));
            }
        }
        for deployment in &self.deployments {
            if !model_ids.contains(deployment.logical_model_id.as_str())
                || !pool_ids.contains(deployment.capacity_pool_id.as_str())
            {
                return Err(invalid("deployment references an unknown model or pool"));
            }
            valid_text(&deployment.label, 256)?;
            valid_text(&deployment.provider_model, 256)?;
            valid_text(&deployment.endpoint, 2048)?;
            let endpoint = reqwest::Url::parse(&deployment.endpoint)
                .map_err(|_| invalid("invalid endpoint URL"))?;
            if !matches!(endpoint.scheme(), "http" | "https")
                || endpoint.host_str().is_none()
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
                || endpoint.query().is_some()
                || endpoint.fragment().is_some()
            {
                return Err(invalid(
                    "endpoint must be HTTP(S), without credentials, query or fragment",
                ));
            }
            if crate::provider::ProviderProfile::parse(&deployment.provider_profile).is_none() {
                return Err(invalid("unsupported provider profile"));
            }
        }
        let policy = &self.policy;
        if policy.global_max_concurrent == 0
            || policy.global_max_concurrent > 1024
            || !(1_000..=3_600_000).contains(&policy.permit_ttl_ms)
            || !(1_000..=3_600_000).contains(&policy.heartbeat_stale_ms)
            || !(1_000..=86_400_000).contains(&policy.lease_ttl_ms)
        {
            return Err(invalid("invalid capacity or timeout policy"));
        }
        Ok(())
    }
}

fn unique_ids<'a>(values: impl Iterator<Item = &'a str>) -> Result<BTreeSet<&'a str>, HubError> {
    let mut ids = BTreeSet::new();
    for id in values {
        if !valid_token(id, 128) {
            return Err(invalid("invalid stable identity token"));
        }
        if !ids.insert(id) || ids.len() > MAX_CATALOG_ENTRIES {
            return Err(invalid("duplicate IDs or catalog exceeds entry limit"));
        }
    }
    Ok(ids)
}

fn valid_text(value: &str, max: usize) -> Result<(), HubError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(invalid(
            "invalid empty, oversized or control-character text",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn valid_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn invalid(message: &str) -> HubError {
    HubError::InvalidCatalog {
        message: message.to_owned(),
    }
}
