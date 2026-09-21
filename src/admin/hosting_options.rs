//! Local interface suggestions only: no LAN scanning or implicit public bind.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::Ipv4Addr;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Candidate {
    pub(super) label: String,
    pub(super) ip: Ipv4Addr,
    #[serde(default)]
    prefix_length: Option<u32>,
    #[serde(default)]
    pub(super) profile: Option<String>,
}

impl Candidate {
    pub(super) fn trusted_scope(&self) -> Option<(String, String)> {
        let prefix = self
            .prefix_length
            .filter(|value| (8..=32).contains(value))?;
        let profile = match self.profile.as_deref()? {
            "DomainAuthenticated" => "Domain",
            "Private" => "Private",
            _ => return None,
        };
        let mask = u32::MAX << (32 - prefix);
        let start = u32::from(self.ip) & mask;
        let end = start | !mask;
        if !Ipv4Addr::from(start).is_private() || !Ipv4Addr::from(end).is_private() {
            return None;
        }
        Some((
            format!("{}/{prefix}", Ipv4Addr::from(start)),
            profile.into(),
        ))
    }
}

fn candidates(bytes: &[u8]) -> Result<Vec<Candidate>, String> {
    if bytes.len() > 64 * 1024 {
        return Err("LAN候補の応答が上限を超えています。IPv4を手入力してください。".into());
    }
    let mut rows: Vec<Candidate> = serde_json::from_slice(bytes)
        .map_err(|_| "LAN候補を読み取れません。IPv4を手入力してください。")?;
    rows.retain(|row| {
        !row.ip.is_loopback()
            && !row.ip.is_unspecified()
            && !row.ip.is_multicast()
            && !row.ip.is_link_local()
            && row.label.len() <= 1024
    });
    rows.sort_by(|a, b| a.label.cmp(&b.label).then(a.ip.cmp(&b.ip)));
    rows.dedup_by(|a, b| a.ip == b.ip);
    rows.truncate(64);
    Ok(rows)
}

#[cfg(windows)]
pub(super) async fn inspect() -> Result<Vec<Candidate>, String> {
    use std::process::Stdio;
    // Fixed local OS query; neither browser input nor shell fragments are accepted.
    let script = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $profiles=@(Get-NetConnectionProfile); $rows=@(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.AddressState -eq 'Preferred' } | Select-Object -First 64 | ForEach-Object { $index=$_.InterfaceIndex; $profile=@($profiles | Where-Object { $_.InterfaceIndex -eq $index }); @{label=$_.InterfaceAlias;ip=$_.IPAddress;prefix_length=$_.PrefixLength;profile=if ($profile.Count -eq 1) { [string]$profile[0].NetworkCategory } else { $null }} }); ConvertTo-Json -InputObject $rows -Compress";
    let system = std::env::var_os("SystemRoot").ok_or("Windowsの場所を確認できません。")?;
    let mut command = tokio::process::Command::new(
        std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    );
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(8), command.output())
        .await
        .map_err(|_| "LAN候補の確認が時間内に終わりません。IPv4を手入力してください。")?
        .map_err(|_| "LAN候補を確認できません。IPv4を手入力してください。")?;
    if !output.status.success() {
        return Err("LAN候補を確認できません。IPv4を手入力してください。".into());
    }
    candidates(&output.stdout)
}

#[cfg(not(windows))]
pub(super) async fn inspect() -> Result<Vec<Candidate>, String> {
    Err("このOSではLAN候補の自動取得に対応していません。IPv4を手入力してください。".into())
}

pub(super) async fn snapshot() -> Result<Value, String> {
    let (rows, error) = match inspect().await {
        Ok(rows) => (rows, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(json!({"candidates":rows,"error":error,
        "executable":std::env::current_exe().ok().map(|v|v.to_string_lossy().into_owned()),
        "default_hub_port":9471,"default_gateway_port":9472}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interface_candidates_are_local_bounded_and_do_not_choose_for_the_user() {
        let rows = candidates(br#"[{"label":"VPN","ip":"10.0.0.1"},{"label":"LAN","ip":"192.168.1.4"},{"label":"loopback","ip":"127.0.0.1"},{"label":"auto","ip":"169.254.1.1"}]"#).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "LAN");
        assert!(candidates(br#"{}"#).is_err());
    }
    #[test]
    fn scope_suggestions_only_use_observed_trusted_private_subnets() {
        let mut candidate = Candidate {
            label: "LAN".into(),
            ip: "192.168.10.106".parse().unwrap(),
            prefix_length: Some(24),
            profile: Some("Private".into()),
        };
        assert_eq!(
            candidate.trusted_scope(),
            Some(("192.168.10.0/24".into(), "Private".into()))
        );
        candidate.profile = Some("Public".into());
        assert!(candidate.trusted_scope().is_none());
        candidate.profile = Some("DomainAuthenticated".into());
        candidate.prefix_length = Some(8);
        assert!(candidate.trusted_scope().is_none());
        candidate.prefix_length = None;
        assert!(candidate.trusted_scope().is_none());
    }
}
