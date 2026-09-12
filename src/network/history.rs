//! Local-administrator history requests are delivered over the device's existing
//! authenticated outbound connection. The Hub never executes a job or exposes
//! these snapshots to another device. Task bodies are not persisted by the Hub.

use super::{DeviceNetwork, NetworkError, PRESENCE_MS, Result, id_ok};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_HISTORY_BODY_BYTES: usize = 512 * 1024;
const MAX_MARKDOWN_BYTES: usize = 64 * 1024;
const REQUEST_TTL_MS: u64 = 60_000;
const SNAPSHOT_TTL_MS: u64 = 30 * 60_000;
const SNAPSHOT_LIMIT: usize = 64;
const MAX_OFFSET: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    Instruction,
    Execution,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    pub direction: HistoryDirection,
    pub offset: usize,
    pub anchor: Option<String>,
    pub id: Option<String>,
}
impl HistoryQuery {
    fn validate(&self) -> Result<()> {
        if self.offset > MAX_OFFSET
            || (self.offset > 0 && self.anchor.is_none())
            || self
                .anchor
                .as_deref()
                .is_some_and(|anchor| !anchor_ok(anchor, self.direction))
            || self.id.as_deref().is_some_and(|id| !ulid_ok(id))
            || (self.id.is_some() && (self.offset != 0 || self.anchor.is_some()))
        {
            return Err(NetworkError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedHistoryRequest {
    pub request_id: String,
    pub device_id: String,
    pub direction: HistoryDirection,
    pub offset: usize,
    pub anchor: Option<String>,
    pub id: Option<String>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRow {
    pub id: String,
    pub direction: HistoryDirection,
    pub created_at_ms: i64,
    pub updated_at_ms: Option<i64>,
    pub title: String,
    pub peer_label: String,
    pub target_label: String,
    pub state: String,
    pub stop_status: String,
    pub state_source: String,
    pub result_received: bool,
    pub session_id: String,
    pub profile_id: String,
    pub job_id: Option<String>,
    pub root_task_id: String,
    pub device_path: Vec<String>,
    pub can_stop: bool,
}
impl HistoryRow {
    fn validate(&self, direction: HistoryDirection) -> bool {
        self.direction == direction
            && ulid_ok(&self.id)
            && ulid_ok(&self.session_id)
            && self.created_at_ms >= 0
            && self.updated_at_ms.is_none_or(|time| time >= 0)
            && bounded_text(&self.title, 2048)
            && bounded_text(&self.peer_label, 2048)
            && bounded_text(&self.target_label, 2048)
            && id_ok(&self.state)
            && id_ok(&self.stop_status)
            && id_ok(&self.state_source)
            && locator_ok(&self.profile_id)
            && locator_ok(&self.root_task_id)
            && self.job_id.as_deref().is_none_or(locator_ok)
            && self.device_path.len() <= 5
            && self.device_path.iter().all(|id| locator_ok(id))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryPage {
    pub rows: Vec<HistoryRow>,
    pub next_offset: Option<usize>,
    pub anchor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryDetail {
    pub row: HistoryRow,
    pub markdown: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HistoryResponse {
    Page(HistoryPage),
    Detail(HistoryDetail),
}
impl HistoryResponse {
    fn validate(&self, request: &QueuedHistoryRequest) -> bool {
        match (self, &request.id) {
            (Self::Page(page), None) => {
                page.rows.len() <= 100
                    && page.rows.iter().all(|row| row.validate(request.direction))
                    && page
                        .rows
                        .iter()
                        .map(|row| &row.id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == page.rows.len()
                    && page
                        .anchor
                        .as_deref()
                        .is_none_or(|anchor| anchor_ok(anchor, request.direction))
                    && (page.rows.is_empty() || page.anchor.is_some())
                    && request
                        .anchor
                        .as_ref()
                        .is_none_or(|anchor| page.anchor.as_ref() == Some(anchor))
                    && page.next_offset.is_none_or(|next| {
                        !page.rows.is_empty()
                            && next == request.offset + page.rows.len()
                            && next <= MAX_OFFSET
                    })
            }
            (Self::Detail(detail), Some(id)) => {
                detail.row.id == *id
                    && detail.row.validate(request.direction)
                    && bounded_text(&detail.markdown, MAX_MARKDOWN_BYTES)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySubmission {
    pub request_id: String,
    pub response: Option<HistoryResponse>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    Pending,
    Completed,
    Failed,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistorySnapshot {
    pub request_id: String,
    pub device_id: String,
    pub device_label: String,
    pub direction: HistoryDirection,
    pub offset: usize,
    pub anchor: Option<String>,
    pub id: Option<String>,
    pub status: HistoryStatus,
    pub requested_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub expires_at_ms: u64,
    pub response: Option<HistoryResponse>,
    pub error: Option<String>,
    pub device_online: bool,
    pub device_admission: String,
    /// Even an online response is a snapshot, never a live execution assertion.
    pub cached: bool,
}

struct Entry {
    request: QueuedHistoryRequest,
    label: String,
    requested_at_ms: u64,
    completed_at_ms: Option<u64>,
    status: HistoryStatus,
    response: Option<HistoryResponse>,
    error: Option<String>,
}
#[derive(Default)]
pub(super) struct HistoryBroker {
    entries: BTreeMap<String, Entry>,
}
impl HistoryBroker {
    fn prune(&mut self, now: u64) {
        for entry in self.entries.values_mut() {
            if entry.status == HistoryStatus::Pending && now >= entry.request.expires_at_ms {
                entry.status = HistoryStatus::Expired;
                entry.completed_at_ms = Some(entry.request.expires_at_ms);
                entry.error = Some("history_request_expired".into());
            }
        }
        self.entries.retain(|_, entry| {
            entry
                .completed_at_ms
                .is_none_or(|time| now.saturating_sub(time) < SNAPSHOT_TTL_MS)
        });
        let mut completed: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| entry.completed_at_ms.map(|time| (time, id.clone())))
            .collect();
        completed.sort();
        let surplus = completed.len().saturating_sub(SNAPSHOT_LIMIT);
        for (_, id) in completed.into_iter().take(surplus) {
            self.entries.remove(&id);
        }
    }
}

impl DeviceNetwork {
    /// Queue a device history query through the shared server management owner.
    /// The device returns its result over its authenticated network connection.
    pub fn request_history(
        &mut self,
        device_id: &str,
        query: HistoryQuery,
        now: u64,
    ) -> Result<HistorySnapshot> {
        query.validate()?;
        let label = self.active_device(device_id, now)?.label.clone();
        self.history.prune(now);
        if !self.history_device_online(device_id, now) {
            return Err(NetworkError::Unavailable);
        }
        if let Some(entry) = self.history.entries.values().find(|entry| {
            entry.request.device_id == device_id && entry.status == HistoryStatus::Pending
        }) {
            let request = &entry.request;
            if request.direction == query.direction
                && request.offset == query.offset
                && request.anchor == query.anchor
                && request.id == query.id
            {
                return self.history_result(&request.request_id.clone(), now);
            }
            return Err(NetworkError::Full);
        }
        let request_id = ulid::Ulid::new().to_string();
        self.history.entries.insert(
            request_id.clone(),
            Entry {
                request: QueuedHistoryRequest {
                    request_id: request_id.clone(),
                    device_id: device_id.into(),
                    direction: query.direction,
                    offset: query.offset,
                    anchor: query.anchor,
                    id: query.id,
                    expires_at_ms: now.saturating_add(REQUEST_TTL_MS),
                },
                label,
                requested_at_ms: now,
                completed_at_ms: None,
                status: HistoryStatus::Pending,
                response: None,
                error: None,
            },
        );
        self.history_result(&request_id, now)
    }
    pub fn history_requests(
        &mut self,
        device_id: &str,
        now: u64,
    ) -> Result<Vec<QueuedHistoryRequest>> {
        self.active_device(device_id, now)?;
        self.history.prune(now);
        Ok(self
            .history
            .entries
            .values()
            .filter(|entry| {
                entry.request.device_id == device_id && entry.status == HistoryStatus::Pending
            })
            .map(|entry| entry.request.clone())
            .collect())
    }
    pub fn submit_history(
        &mut self,
        device_id: &str,
        submission: HistorySubmission,
        now: u64,
    ) -> Result<()> {
        self.active_device(device_id, now)?;
        self.history.prune(now);
        let encoded = serde_json::to_vec(&submission).map_err(|_| NetworkError::InvalidRequest)?;
        if encoded.len() > MAX_HISTORY_BODY_BYTES {
            return Err(NetworkError::Full);
        }
        let entry = self
            .history
            .entries
            .get_mut(&submission.request_id)
            .ok_or(NetworkError::InvalidRequest)?;
        if entry.request.device_id != device_id {
            return Err(NetworkError::Unauthorized);
        }
        let shape_valid = match (&submission.response, &submission.error) {
            (Some(response), None) => response.validate(&entry.request),
            (None, Some(error)) => {
                !error.is_empty()
                    && bounded_text(error, 256)
                    && !error.chars().any(char::is_control)
            }
            _ => false,
        };
        if !shape_valid {
            return Err(NetworkError::InvalidRequest);
        }
        if matches!(
            entry.status,
            HistoryStatus::Completed | HistoryStatus::Failed
        ) && entry.response == submission.response
            && entry.error == submission.error
        {
            return Ok(());
        }
        if entry.status != HistoryStatus::Pending {
            return Err(NetworkError::StaleRevision);
        }
        entry.status = if submission.error.is_some() {
            HistoryStatus::Failed
        } else {
            HistoryStatus::Completed
        };
        entry.completed_at_ms = Some(now);
        entry.response = submission.response;
        entry.error = submission.error;
        self.history.prune(now);
        Ok(())
    }
    /// The Hub retains snapshots locally, including when a device is offline.
    pub fn history_result(&mut self, request_id: &str, now: u64) -> Result<HistorySnapshot> {
        if !ulid_ok(request_id) {
            return Err(NetworkError::InvalidRequest);
        }
        self.history.prune(now);
        let entry = self
            .history
            .entries
            .get(request_id)
            .ok_or(NetworkError::InvalidRequest)?;
        let device = self.store.devices.get(&entry.request.device_id);
        Ok(HistorySnapshot {
            request_id: request_id.into(),
            device_id: entry.request.device_id.clone(),
            device_label: entry.label.clone(),
            direction: entry.request.direction,
            offset: entry.request.offset,
            anchor: entry.request.anchor.clone(),
            id: entry.request.id.clone(),
            status: entry.status,
            requested_at_ms: entry.requested_at_ms,
            completed_at_ms: entry.completed_at_ms,
            expires_at_ms: entry.request.expires_at_ms,
            response: entry.response.clone(),
            error: entry.error.clone(),
            device_online: self.history_device_online(&entry.request.device_id, now),
            device_admission: match device {
                Some(device) if device.revoked => "revoked",
                Some(device) if device.stopped => "stopped",
                Some(_) => "allowed",
                None => "unavailable",
            }
            .into(),
            cached: entry.response.is_some(),
        })
    }
    fn history_device_online(&self, device_id: &str, now: u64) -> bool {
        self.active_device(device_id, now).is_ok()
            && self
                .presence
                .get(device_id)
                .is_some_and(|p| now.saturating_sub(p.last_seen_ms) <= PRESENCE_MS)
    }
}

fn bounded_text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}
fn locator_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn ulid_ok(value: &str) -> bool {
    ulid::Ulid::from_string(value).is_ok_and(|id| id.to_string() == value)
}
fn anchor_ok(value: &str, direction: HistoryDirection) -> bool {
    if value.len() > 64 {
        return false;
    }
    let prefix = match direction {
        HistoryDirection::Instruction => "instruction",
        HistoryDirection::Execution => "execution",
    };
    value
        .split_once(':')
        .is_some_and(|(kind, rowid)| kind == prefix && rowid.parse::<i64>().is_ok_and(|id| id > 0))
}
