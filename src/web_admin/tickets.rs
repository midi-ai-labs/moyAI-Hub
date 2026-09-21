use super::*;
use crate::identity::web_access::DeviceProof;
use ring::rand::{SecureRandom, SystemRandom};

pub(super) struct Ticket {
    digest: [u8; 32],
    origin: String,
    pub(super) expires: std::time::Instant,
    pub user_id: String,
    pub device: Option<DeviceProof>,
}

impl WebAccess {
    pub(super) fn set_remote_origin(&self, origin: Option<String>) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "管理画面の状態を確認できません。")?
            .remote_origin = origin;
        Ok(())
    }
    pub(crate) fn remote_ticket(
        &self,
        user_id: String,
        proof: DeviceProof,
    ) -> Result<String, String> {
        let origin = self
            .state
            .lock()
            .map_err(|_| "管理画面の状態を確認できません。")?
            .remote_origin
            .clone()
            .ok_or("Hub PC の管理画面で、HTTPSによるリモート管理を有効にしてください。")?;
        self.issue_ticket(&origin, user_id, Some(proof))
    }
    pub(super) fn issue_ticket(
        &self,
        origin: &str,
        user_id: String,
        device: Option<DeviceProof>,
    ) -> Result<String, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "管理画面の状態を確認できません。")?;
        if state.epoch.is_none() {
            return Err("管理画面は停止中です。".into());
        }
        let now = std::time::Instant::now();
        state.tickets.retain(|v| v.expires > now);
        if state.tickets.len() >= 32 {
            return Err(
                "管理画面を開く処理が多すぎます。少し待ってから開き直してください。".into(),
            );
        }
        let mut bytes = [0u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| "管理画面の認証を準備できません。")?;
        let ticket: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
        state.tickets.push(Ticket {
            digest: Sha256::digest(ticket.as_bytes()).into(),
            origin: origin.into(),
            expires: now + Duration::from_secs(60),
            user_id,
            device,
        });
        Ok(format!("{origin}/admin/#access={ticket}"))
    }
    pub(super) fn consume_ticket(
        &self,
        value: &str,
        policy: &WebPolicy,
    ) -> Result<Ticket, StatusCode> {
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if state.epoch.as_deref() != Some(&policy.access_id) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let digest: [u8; 32] = Sha256::digest(value.as_bytes()).into();
        let now = std::time::Instant::now();
        state.tickets.retain(|v| v.expires > now);
        let index = state
            .tickets
            .iter()
            .position(|v| v.origin == policy.origin && equal(&v.digest, &digest))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        Ok(state.tickets.swap_remove(index))
    }
}
