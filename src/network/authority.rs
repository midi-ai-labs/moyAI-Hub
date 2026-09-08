//! Durable authorization metadata; never stores task text or bearer credentials.
use super::*;

const ACTIVE_ROOT_LIMIT: usize = 128;
const AUTHORITY_LIMIT: usize = 4096;
const TERMINAL_ROOT_LIMIT: usize = 128;
const OPAQUE_RETIREMENT_LIMIT: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Authority {
    pub id: String,
    pub claims: Claims,
    pub parent_grant_id: Option<String>,
    pub policy_hash: String,
    pub execute_retired: bool,
    pub job_id: Option<String>,
    pub terminal: Option<Terminal>,
    pub created_at_ms: u64,
    pub settled_at_ms: Option<u64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Terminal {
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ledger {
    pub authorities: BTreeMap<String, Authority>,
    pub roots: BTreeMap<String, Root>,
    pub retired_before: BTreeMap<String, String>,
    pub retired_opaque: BTreeSet<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub origin: String,
    pub task: String,
    pub cancelled: bool,
}
impl Ledger {
    pub fn key(origin: &str, task: &str) -> String {
        format!("{origin}/{task}")
    }
    pub fn retired(&self, origin: &str, task: &str) -> bool {
        let key = Self::key(origin, task);
        !self.roots.contains_key(&key)
            && (self.retired_opaque.contains(&key)
                || (task
                    .parse::<ulid::Ulid>()
                    .ok()
                    .is_some_and(|id| id.to_string() == task)
                    && self
                        .retired_before
                        .get(origin)
                        .is_some_and(|value| task <= value.as_str())))
    }
    pub fn cancelled(&self, claims: &Claims) -> bool {
        self.roots
            .get(&Self::key(&claims.origin_device_id, &claims.root_task_id))
            .is_some_and(|root| root.cancelled)
    }
    pub fn validate(&self) -> Result<()> {
        if self.authorities.len() > AUTHORITY_LIMIT * 2
            || self.roots.len() > ACTIVE_ROOT_LIMIT + TERMINAL_ROOT_LIMIT
            || self.retired_before.len() > DEVICE_LIMIT
            || self.retired_opaque.len() > OPAQUE_RETIREMENT_LIMIT
        {
            return Err(NetworkError::InvalidRequest);
        }
        for (key, root) in &self.roots {
            if key != &Self::key(&root.origin, &root.task)
                || !id_ok(&root.origin)
                || !id_ok(&root.task)
            {
                return Err(NetworkError::InvalidRequest);
            }
        }
        let mut accepted_jobs = BTreeSet::new();
        for (id, a) in &self.authorities {
            if let Some(job) = &a.job_id {
                if !accepted_jobs.insert((&a.claims.audience_device_id, job)) {
                    return Err(NetworkError::InvalidRequest);
                }
            }
            if id != &a.id
                || !id_ok(id)
                || !self.roots.contains_key(&Self::key(
                    &a.claims.origin_device_id,
                    &a.claims.root_task_id,
                ))
                || a.claims.depth == 0
                || a.claims.depth > 4
                || a.claims.device_path.len() != usize::from(a.claims.depth) + 1
                || a.parent_grant_id
                    .as_ref()
                    .is_some_and(|p| !self.authorities.contains_key(p))
            {
                return Err(NetworkError::InvalidRequest);
            }
            let c = &a.claims;
            if [
                &c.hub_id,
                &c.origin_device_id,
                &c.actor_device_id,
                &c.audience_device_id,
                &c.profile_id,
                &c.scope_id,
                &c.root_task_id,
                &c.request_key,
            ]
            .into_iter()
            .any(|id| !id_ok(id))
                || a.job_id.as_ref().is_some_and(|id| !id_ok(id))
                || c.device_path.first() != Some(&c.origin_device_id)
                || c.device_path.last() != Some(&c.audience_device_id)
                || c.device_path.get(c.device_path.len() - 2) != Some(&c.actor_device_id)
                || c.device_path.iter().collect::<BTreeSet<_>>().len() != c.device_path.len()
                || a.terminal.is_some() != a.settled_at_ms.is_some()
            {
                return Err(NetworkError::InvalidRequest);
            }
            if let Some(parent_id) = &a.parent_grant_id {
                let parent = &self.authorities[parent_id];
                if parent.claims.root_task_id != c.root_task_id
                    || parent.claims.origin_device_id != c.origin_device_id
                    || parent.claims.audience_device_id != c.actor_device_id
                    || parent.job_id.is_none()
                    || parent.job_id != c.parent_job_id
                    || c.depth != parent.claims.depth + 1
                    || c.device_path[..c.device_path.len() - 1] != parent.claims.device_path
                {
                    return Err(NetworkError::InvalidRequest);
                }
            } else if c.depth != 1 || c.parent_job_id.is_some() {
                return Err(NetworkError::InvalidRequest);
            }
        }
        Ok(())
    }
    pub fn cleanup(&mut self, now: u64) {
        // A root is compacted only when every accepted descendant has settled.
        // Active work has no elapsed-time expiry. The watermark rejects replay
        // after removing terminal records, including reused request keys.
        let mut finished: Vec<_> = self
            .roots
            .iter()
            .filter_map(|(key, root)| {
                let rows: Vec<_> = self
                    .authorities
                    .values()
                    .filter(|a| {
                        a.claims.origin_device_id == root.origin
                            && a.claims.root_task_id == root.task
                    })
                    .collect();
                if rows.is_empty() || rows.iter().any(|a| a.terminal.is_none()) {
                    return None;
                }
                Some((
                    key.clone(),
                    rows.iter()
                        .filter_map(|a| a.settled_at_ms)
                        .max()
                        .unwrap_or(now),
                ))
            })
            .collect();
        finished.sort_by_key(|(_, time)| *time);
        let excess = finished.len().saturating_sub(TERMINAL_ROOT_LIMIT);
        for (index, (key, time)) in finished.into_iter().enumerate() {
            if index >= excess && now.saturating_sub(time) < RETENTION_MS {
                continue;
            }
            let root = &self.roots[&key];
            let canonical = root
                .task
                .parse::<ulid::Ulid>()
                .ok()
                .is_some_and(|id| id.to_string() == root.task);
            if canonical {
                let watermark = self.retired_before.entry(root.origin.clone()).or_default();
                if root.task > *watermark {
                    *watermark = root.task.clone();
                }
            } else {
                if self.retired_opaque.len() >= OPAQUE_RETIREMENT_LIMIT {
                    continue;
                }
                self.retired_opaque.insert(key.clone());
            }
            let origin = root.origin.clone();
            let task = root.task.clone();
            self.authorities.retain(|_, a| {
                a.claims.origin_device_id != origin || a.claims.root_task_id != task
            });
            self.roots.remove(&key);
        }
    }
    pub fn insert(&mut self, authority: Authority, now: u64) -> Result<()> {
        self.cleanup(now);
        let c = &authority.claims;
        let key = Self::key(&c.origin_device_id, &c.root_task_id);
        if self.retired(&c.origin_device_id, &c.root_task_id) {
            return Err(NetworkError::AuthorityRetired);
        }
        if self.authorities.len() >= AUTHORITY_LIMIT * 2
            || self
                .authorities
                .values()
                .filter(|a| a.terminal.is_none())
                .count()
                >= AUTHORITY_LIMIT
            || self
                .authorities
                .values()
                .filter(|a| {
                    a.claims.origin_device_id == c.origin_device_id
                        && a.claims.root_task_id == c.root_task_id
                })
                .count()
                >= 32
        {
            return Err(NetworkError::Full);
        }
        if !self.roots.contains_key(&key) {
            let active = self
                .roots
                .values()
                .filter(|r| {
                    self.authorities.values().any(|a| {
                        a.claims.origin_device_id == r.origin
                            && a.claims.root_task_id == r.task
                            && a.terminal.is_none()
                    })
                })
                .count();
            if active >= ACTIVE_ROOT_LIMIT
                || self.roots.len() >= ACTIVE_ROOT_LIMIT + TERMINAL_ROOT_LIMIT
            {
                return Err(NetworkError::Full);
            }
            self.roots.insert(
                key,
                Root {
                    origin: c.origin_device_id.clone(),
                    task: c.root_task_id.clone(),
                    cancelled: false,
                },
            );
        }
        self.authorities.insert(authority.id.clone(), authority);
        Ok(())
    }
}
