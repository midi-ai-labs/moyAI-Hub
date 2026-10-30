//! Removal remains possible after migration, even when the old executable is gone.
use super::*;

#[derive(Serialize)]
struct RetiredRule {
    spec: RuleSpec,
    profile_directory: String,
    removable: bool,
    fingerprint: String,
}

fn select(
    rows: Vec<Observed>,
    profiles: &[PathBuf],
    program: &str,
    binary_sha256: &str,
) -> Result<Vec<RetiredRule>, String> {
    let current_name = rule_name(program, &profiles[0].to_string_lossy());
    rows.into_iter()
        .filter(|row| row.spec.name != current_name)
        .filter_map(|row| {
            let directory = profiles
                .iter()
                .find(|directory| profile_has_rule(directory, std::slice::from_ref(&row)))?;
            let profile_directory = directory.to_string_lossy().into_owned();
            Some(
                serde_json::to_vec(&(
                    &row.spec,
                    &profile_directory,
                    row.owned,
                    program,
                    binary_sha256,
                ))
                .map(|value| RetiredRule {
                    spec: row.spec,
                    profile_directory,
                    removable: row.owned,
                    fingerprint: digest(value),
                })
                .map_err(|_| "以前の受信許可を照合できません。".into()),
            )
        })
        .collect()
}

async fn inventory(admin: &Admin) -> Result<(Vec<RetiredRule>, String, String), String> {
    #[cfg(not(windows))]
    {
        let _ = admin;
        Err("受信許可の確認はWindowsのHub PCで利用できます。".into())
    }
    #[cfg(windows)]
    {
        let profiles = crate::data_paths::firewall_profiles(
            admin.path.parent().ok_or("保存先がありません。")?,
        )?;
        let (program, hash) = executable()?;
        let script = format!(
            "{HELPER}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $names=@(Get-NetFirewallRule -PolicyStore PersistentStore | Where-Object {{ $_.Group -ceq 'moyAI Hub managed inbound v1' }} | Select-Object -First 65 -ExpandProperty Name); if ($names.Count -gt 64) {{ throw 'Too many managed rules' }}; $rows=@($names | ForEach-Object {{ Get-MoyaiRule $_ }}); ConvertTo-Json -InputObject $rows -Depth 8 -Compress"
        );
        let result = run_script(&script, 15).await?;
        if !result.status.success() || result.stdout.len() > 256 * 1024 {
            return Err("以前の受信許可を読み取れません。PC管理者へ確認してください。".into());
        }
        let rows = serde_json::from_slice(&result.stdout)
            .map_err(|_| "以前の受信許可の内容を確認できません。")?;
        Ok((select(rows, &profiles, &program, &hash)?, program, hash))
    }
}

pub(in crate::admin) async fn list(admin: &Admin) -> Result<Value, String> {
    let (rows, _, _) = inventory(admin).await?;
    output(rows)
}

pub(in crate::admin) async fn remove(admin: &Admin, expected: String) -> Result<Value, String> {
    // Serialize with normal firewall changes and listener reconfiguration.
    let _server = admin.network_server.lock().await;
    let (rows, program, hash) = inventory(admin).await?;
    let row = rows
        .into_iter()
        .find(|row| row.removable && row.fingerprint == expected)
        .ok_or("以前の受信許可が変わりました。内容を再確認してください。")?;
    let request = json!({"action":"remove_retired", "spec":row.spec, "previous":row.spec,
        "profile_directory":row.profile_directory, "reviewer_program":program,
        "binary_sha256":hash, "expires_at_ms":now_ms()+60_000});
    let body = payload(&request)?;
    let encoded = encoded_script(&format!(
        "{HELPER}\ntry {{ $request=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{body}')) | ConvertFrom-Json; Invoke-MoyaiFirewall $request; exit 0 }} catch {{ exit 22 }}"
    ));
    if encoded.len() > 30_000 {
        return Err("確認内容が上限を超えています。PC管理者へ解除を依頼してください。".into());
    }
    #[cfg(windows)]
    let outcome = run_script(&elevation_script(&encoded), 90)
        .await
        .ok()
        .and_then(|result| result.status.code());
    #[cfg(not(windows))]
    let outcome: Option<i32> = None;
    let verified = observe(&row.spec.name)
        .await
        .is_ok_and(|value| value.is_none());
    Ok(
        json!({"status":if outcome == Some(0) && verified {"verified"} else if outcome == Some(20) {"cancelled"} else {"unconfirmed"}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_rules_are_scoped_to_profile_and_cannot_override_manual_changes() {
        let profiles = vec![PathBuf::from(r"C:\Data\hub"), PathBuf::from(r"C:\OldData")];
        let program = r"C:\Program Files\moyAI\Hub\moyai-hub.exe";
        let make = |program: &str, profile: &Path, owned| {
            let mut spec = super::super::tests::spec();
            spec.program = program.into();
            spec.name = rule_name(program, &profile.to_string_lossy());
            Observed { spec, owned }
        };
        let old = r"C:\Deleted\moyai-hub.exe";
        let rows = vec![
            make(program, &profiles[0], true),
            make(old, &profiles[0], true),
            make(program, &profiles[1], true),
            make(old, Path::new(r"C:\OtherProfile"), true),
            make(old, &profiles[1], false),
        ];
        let selected = select(rows.clone(), &profiles, program, "hash").unwrap();
        assert_eq!(selected.len(), 3);
        assert!(selected[0].removable && selected[1].removable && !selected[2].removable);
        assert_ne!(
            selected[0].fingerprint,
            select(rows, &profiles, program, "changed").unwrap()[0].fingerprint
        );
    }
}
