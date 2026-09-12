//! Device names and irreversible removal share the registry's save boundary.
use super::*;

impl DeviceNetwork {
    pub(super) fn registered_count(store: &Store) -> usize {
        store
            .devices
            .values()
            .filter(|device| device.deleted_at_ms.is_none())
            .count()
    }
    pub(super) fn label_key(label: &str) -> String {
        // Unicode whitespace and lowercase are folded; canonical/compatibility
        // normalization is intentionally not applied (e.g. full-width != ASCII).
        label.trim().to_lowercase()
    }
    pub(super) fn label_conflicts(
        store: &Store,
        label: &str,
        device_id: Option<&str>,
        request_id: Option<&str>,
        now: u64,
    ) -> bool {
        let key = Self::label_key(label);
        store.devices.values().any(|device| {
            device.deleted_at_ms.is_none()
                && Some(device.device_id.as_str()) != device_id
                && Self::label_key(&device.label) == key
        }) || Self::pending_label_conflicts(store, &key, request_id, now)
    }
    pub(super) fn available_label(
        store: &Store,
        label: &str,
        device_id: Option<&str>,
        request_id: Option<&str>,
        now: u64,
    ) -> Result<String> {
        let label = label.trim();
        if !text_ok(label, 256) {
            return Err(NetworkError::InvalidRequest);
        }
        if Self::label_conflicts(store, label, device_id, request_id, now) {
            return Err(NetworkError::DeviceNameConflict);
        }
        Ok(label.into())
    }
    pub fn unsettled_device_jobs(&self, id: &str) -> usize {
        self.store
            .authority
            .authorities
            .values()
            .filter(|authority| {
                authority.job_id.is_some()
                    && authority.terminal.is_none()
                    && authority
                        .claims
                        .device_path
                        .iter()
                        .any(|device| device == id)
            })
            .count()
    }
    /// Registration removal does not stop a remote process. Refuse while any
    /// accepted work is unsettled, including offline or ancestor devices.
    pub fn delete_device(&mut self, expected: &str, id: &str, now: u64) -> Result<()> {
        self.edit(expected, |store| {
            let device = store
                .devices
                .get(id)
                .filter(|device| device.deleted_at_ms.is_none())
                .ok_or(NetworkError::InvalidRequest)?;
            if store.authority.authorities.values().any(|authority| {
                authority.job_id.is_some()
                    && authority.terminal.is_none()
                    && authority
                        .claims
                        .device_path
                        .iter()
                        .any(|device| device == id)
            }) {
                return Err(NetworkError::DeviceWorkUnsettled);
            }
            if store
                .devices
                .values()
                .filter(|device| device.deleted_at_ms.is_some())
                .count()
                >= DELETED_DEVICE_LIMIT
            {
                return Err(NetworkError::Full);
            }
            let _ = device;
            let device = store
                .devices
                .get_mut(id)
                .ok_or(NetworkError::InvalidRequest)?;
            device.deleted_at_ms = Some(now);
            device.revoked = true;
            device.stopped = true;
            device.groups.clear();
            device.admission_revision = store.revision.checked_add(1).ok_or(NetworkError::Full)?;
            store.rules.retain(|_, rule| {
                ![&rule.from, &rule.to].iter().any(
                    |subject| matches!(subject, Subject::Device { id: target } if target == id),
                )
            });
            store.invitations.retain(|invitation| {
                !invitation
                    .redeemed
                    .as_ref()
                    .is_some_and(|replay| replay.device_id == id)
            });
            for authority in store
                .authority
                .authorities
                .values_mut()
                .filter(|authority| {
                    authority
                        .claims
                        .device_path
                        .iter()
                        .any(|device| device == id)
                })
            {
                authority.execute_retired = true;
                if authority.job_id.is_none() && authority.terminal.is_none() {
                    authority.terminal = Some(Terminal::Cancelled);
                    authority.settled_at_ms = Some(now);
                }
            }
            Ok(())
        })?;
        self.presence.remove(id);
        self.grants
            .retain(|_, grant| !grant.claims.device_path.iter().any(|device| device == id));
        Ok(())
    }
}
