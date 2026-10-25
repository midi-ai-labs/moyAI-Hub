//! Explicit, locally reviewed inbound Windows rules. The OS rule is the state owner.
use super::*;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::net::Ipv4Addr;

const HELPER: &str = include_str!("firewall_helper.ps1");
const EXPORT_HELPER: &str = include_str!("firewall_export.ps1");

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    remote_addresses: Vec<String>,
    profiles: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RuleSpec {
    name: String,
    program: String,
    local_address: Ipv4Addr,
    local_ports: Vec<u16>,
    remote_addresses: Vec<String>,
    profile_mask: u8,
}

#[derive(Clone, Deserialize, Serialize)]
struct Observed {
    spec: RuleSpec,
    owned: bool,
}

#[derive(Serialize)]
struct Preview {
    spec: RuleSpec,
    binary_sha256: String,
    previous: Option<RuleSpec>,
    conflict: bool,
    matching: bool,
    fingerprint: String,
}

fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn private(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

fn subnet(value: &str) -> Result<String, String> {
    let (ip, prefix) = value.split_once('/').unwrap_or((value, "32"));
    let ip: Ipv4Addr = ip
        .parse()
        .map_err(|_| "接続元は社内IPv4またはCIDRで指定してください。")?;
    let prefix: u32 = prefix
        .parse()
        .map_err(|_| "CIDRの範囲を確認してください。")?;
    if !(8..=32).contains(&prefix) {
        return Err("接続元はRFC1918の社内IPv4範囲に限定してください。".into());
    }
    let mask = u32::MAX << (32 - prefix);
    let start = u32::from(ip) & mask;
    let end = start | !mask;
    if !private(Ipv4Addr::from(start)) || !private(Ipv4Addr::from(end)) {
        return Err("接続元は10/8、172.16/12、192.168/16内に限定してください。".into());
    }
    if start != u32::from(ip) {
        return Err(format!(
            "サブネットの先頭を指定してください（{}/{prefix}）。",
            Ipv4Addr::from(start)
        ));
    }
    Ok(format!("{ip}/{prefix}"))
}

fn normalize(scope: Scope) -> Result<(Vec<String>, u8), String> {
    if scope.remote_addresses.is_empty() || scope.remote_addresses.len() > 16 {
        return Err("許可する接続元を1〜16件指定してください。".into());
    }
    let mut remote = scope
        .remote_addresses
        .iter()
        .map(|v| subnet(v.trim()))
        .collect::<Result<Vec<_>, _>>()?;
    remote.sort();
    remote.dedup();
    let mut mask = 0;
    for profile in &scope.profiles {
        mask |= match profile.as_str() {
            "Domain" => 1,
            "Private" => 2,
            _ => return Err("DomainまたはPrivateプロファイルを指定してください。".into()),
        };
    }
    if mask == 0 || scope.profiles.len() > 2 {
        return Err("信頼するネットワークプロファイルを選択してください。".into());
    }
    Ok((remote, mask))
}

fn rule_name(program: &str, directory: &str) -> String {
    let identity = format!(
        "{}\n{}",
        program.to_lowercase(),
        directory
            .replace('/', "\\")
            .trim_start_matches("\\\\?\\")
            .to_lowercase()
    );
    format!("moyai-hub-inbound-v1-{}", &digest(identity)[..24])
}

fn executable() -> Result<(String, String), String> {
    let path = std::env::current_exe().map_err(|_| "実行ファイルを確認できません。")?;
    let path = std::fs::canonicalize(path).map_err(|_| "実行ファイルを確認できません。")?;
    let program = path
        .to_str()
        .ok_or("実行ファイルのパスを確認できません。")?
        .trim_start_matches("\\\\?\\")
        .replace('/', "\\");
    if program.contains(['"', '\r', '\n']) || program.len() > 2048 {
        return Err("実行ファイルのパスを確認してください。".into());
    }
    let bytes = std::fs::read(&path).map_err(|_| "実行ファイルを読み取れません。")?;
    Ok((program, digest(bytes)))
}

fn preview_value(
    spec: RuleSpec,
    binary_sha256: String,
    observed: Option<Observed>,
) -> Result<Preview, String> {
    let conflict = observed.as_ref().is_some_and(|row| {
        !row.owned
            || row.spec.name != spec.name
            || !row.spec.program.eq_ignore_ascii_case(&spec.program)
    });
    let matching = observed.as_ref().is_some_and(|row| {
        let mut previous = row.spec.clone();
        previous.program = spec.program.clone();
        row.owned && row.spec.program.eq_ignore_ascii_case(&spec.program) && previous == spec
    });
    let previous = observed.map(|row| row.spec);
    let fingerprint = digest(
        serde_json::to_vec(&(&spec, &binary_sha256, &previous, conflict))
            .map_err(|_| "確認内容を作成できません。")?,
    );
    Ok(Preview {
        spec,
        binary_sha256,
        previous,
        conflict,
        matching,
        fingerprint,
    })
}

fn encoded_script(script: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
}

fn payload(value: &impl Serialize) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(value).map_err(|_| "確認内容を作成できません。")?))
}

fn elevation_script(encoded: &str) -> String {
    format!(
        "try {{ $p=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe') -ArgumentList '-NoProfile -NonInteractive -EncodedCommand {encoded}' -Verb RunAs -WindowStyle Hidden -PassThru -Wait; exit $p.ExitCode }} catch {{ $cause=$_.Exception; for($i=0; $i -lt 4 -and $null -ne $cause; $i++) {{ if ($cause -is [ComponentModel.Win32Exception] -and $cause.NativeErrorCode -eq 1223) {{ exit 20 }}; $cause=$cause.InnerException }}; exit 23 }}"
    )
}

#[cfg(windows)]
fn powershell() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windowsの場所を確認できません。")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
}

#[cfg(windows)]
async fn run_script(script: &str, timeout: u64) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(powershell()?)
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "Windowsファイアウォールの設定を読み取れません。")?;
    let mut input = child.stdin.take().ok_or("確認処理の入力を開けません。")?;
    // Windows PowerShell's line-oriented -Command - can silently leave a
    // multiline function incomplete. Send one fixed decoding expression;
    // the decoded source is our embedded asset, never browser-provided code.
    let encoded = encoded_script(script);
    let input_script = format!(
        "& ([scriptblock]::Create([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{encoded}'))))\n"
    );
    input
        .write_all(input_script.as_bytes())
        .await
        .map_err(|_| "確認内容を送れません。")?;
    drop(input);
    tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "操作の結果を確認できません。再実行せず、現在のルールを再確認してください。")?
    .map_err(|_| "操作の結果を読み取れません。現在のルールを再確認してください。".into())
}

fn observation_script(name: &str) -> String {
    format!(
        "{HELPER}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); [pscustomobject]@{{observed=(Get-MoyaiRule '{name}')}} | ConvertTo-Json -Depth 8 -Compress"
    )
}

fn decode_observation(bytes: &[u8]) -> Result<Option<Observed>, String> {
    if bytes.len() > 32 * 1024 {
        return Err("ファイアウォールからの応答が大きすぎるため、読み取れません。".into());
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        observed: Value,
    }
    let invalid = || {
        "既存のファイアウォールのルールと照合できません。PC管理者に確認してください。".to_owned()
    };
    let packet: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    serde_json::from_value(packet.observed).map_err(|_| invalid())
}

async fn observe(name: &str) -> Result<Option<Observed>, String> {
    #[cfg(windows)]
    {
        let script = observation_script(name);
        let result = run_script(&script, 15).await?;
        if !result.status.success() || result.stdout.len() > 32 * 1024 {
            return Err(
                "ファイアウォールのルールを読み取れません。PC管理者に確認してください。".into(),
            );
        }
        decode_observation(&result.stdout)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err("ファイアウォールの設定はWindowsのHub PCで利用できます。".into())
    }
}

fn listener_target(endpoints: (&str, &str, &str)) -> Result<(Ipv4Addr, Vec<u16>), String> {
    let (hub, gateway, advertised) = endpoints;
    let hub = reqwest::Url::parse(hub).map_err(|_| "稼働中のHubを確認できません。")?;
    let gateway = reqwest::Url::parse(gateway).map_err(|_| "稼働中のGatewayを確認できません。")?;
    let local_address: Ipv4Addr = advertised
        .parse()
        .map_err(|_| "この設定は社内IPv4のHubで利用できます。")?;
    if !private(local_address)
        || [&hub, &gateway]
            .iter()
            .any(|url| url.scheme() != "https" || url.host_str() != Some(advertised))
    {
        return Err(
            "この設定は社内IPv4のHubとGatewayに限定しています。PC管理者へ設定を依頼してください。"
                .into(),
        );
    }
    let mut local_ports = vec![
        hub.port_or_known_default()
            .ok_or("Hubのポートを確認できません。")?,
        gateway
            .port_or_known_default()
            .ok_or("Gatewayのポートを確認できません。")?,
    ];
    local_ports.sort();
    local_ports.dedup();
    if local_ports.len() != 2 || local_ports.contains(&0) {
        return Err("HubとGatewayのポートを確認してください。".into());
    }
    Ok((local_address, local_ports))
}

async fn prepare(
    admin: &Admin,
    scope: Scope,
    endpoints: (&str, &str, &str),
) -> Result<Preview, String> {
    let (remote_addresses, profile_mask) = normalize(scope)?;
    let (local_address, local_ports) = listener_target(endpoints)?;
    let (program, binary_sha256) = executable()?;
    let directory = std::fs::canonicalize(admin.path.parent().ok_or("保存先がありません。")?)
        .map_err(|_| "保存先を確認できません。")?;
    let name = rule_name(
        &program,
        directory.to_str().ok_or("保存先を確認できません。")?,
    );
    let spec = RuleSpec {
        name,
        program,
        local_address,
        local_ports,
        remote_addresses,
        profile_mask,
    };
    let observed = observe(&spec.name).await?;
    preview_value(spec, binary_sha256, observed)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    current: Option<Observed>,
    other: Vec<Observed>,
}

fn profile_has_rule(directory: &Path, rows: &[Observed]) -> bool {
    rows.iter()
        .any(|row| row.spec.name == rule_name(&row.spec.program, &directory.to_string_lossy()))
}

/// Data relocation/reset must not orphan a rule whose identity includes the old profile path.
pub fn require_profile_firewall_removed(directory: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let directory =
            std::fs::canonicalize(directory).map_err(|_| "Hubの保存先を確認できません。")?;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            runtime.block_on(async {
                let script = format!("{HELPER}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $names=@(Get-NetFirewallRule -PolicyStore PersistentStore | Where-Object {{ $_.Group -ceq 'moyAI Hub managed inbound v1' }} | Select-Object -First 65 -ExpandProperty Name); if ($names.Count -gt 64) {{ throw 'Too many managed rules' }}; $rows=@($names | ForEach-Object {{ Get-MoyaiRule $_ }}); ConvertTo-Json -InputObject $rows -Depth 8 -Compress");
                let result = run_script(&script, 15).await?;
                if !result.status.success() || result.stdout.len() > 256 * 1024 { return Err("Hubの受信許可を確認できません。保存先の移動・初期化は行っていません。PC管理者へ確認してください。".into()); }
                let rows: Vec<Observed> = serde_json::from_slice(&result.stdout).map_err(|_| "Hubの受信許可を照合できません。保存データは保持しています。")?;
                if profile_has_rule(&directory, &rows) { return Err("このHub保存先のWindows受信許可が残っています。元のHubを起動し、管理画面で「表示中の既存ルールを解除」を実行してからHubを停止し、もう一度操作してください。保存先の移動・初期化は行っていません。".into()); }
                Ok(())
            })
        }).join().map_err(|_| "Hub受信許可の確認処理を完了できません。")?
    }
    #[cfg(not(windows))]
    {
        let _ = directory;
        Ok(())
    }
}

fn status_value(
    program: &str,
    local_address: Ipv4Addr,
    local_ports: &[u16],
    inventory: Inventory,
    lan_scope: Option<Scope>,
    active_profile: Option<&str>,
) -> Value {
    let profile_mask = match active_profile {
        Some("DomainAuthenticated") => Some(1),
        Some("Private") => Some(2),
        Some("Public") => Some(4),
        _ => None,
    };
    let state = match inventory.current.as_ref() {
        None => "missing",
        Some(row) if !row.owned || !row.spec.program.eq_ignore_ascii_case(program) => "conflict",
        Some(row)
            if row.spec.local_address == local_address
                && row.spec.local_ports == local_ports
                && profile_mask.is_none_or(|mask| row.spec.profile_mask & mask != 0) =>
        {
            "matching"
        }
        Some(_) => "mismatch",
    };
    let existing_scope = inventory
        .current
        .as_ref()
        .filter(|row| row.owned && row.spec.program.eq_ignore_ascii_case(program))
        .map(|row| Scope {
            remote_addresses: row.spec.remote_addresses.clone(),
            profiles: [(1, "Domain"), (2, "Private")]
                .into_iter()
                .filter(|(mask, _)| row.spec.profile_mask & mask != 0)
                .map(|(_, label)| label.to_owned())
                .collect(),
        });
    let suggestion_source = if existing_scope.is_some() {
        "existing"
    } else {
        "lan"
    };
    let mut other_programs: Vec<_> = inventory
        .other
        .into_iter()
        .filter(|row| row.owned && !row.spec.program.eq_ignore_ascii_case(program))
        .map(|row| row.spec.program)
        .collect();
    other_programs.sort();
    other_programs.dedup();
    json!({"state":state,"program":program,"local_address":local_address,"local_ports":local_ports,"active_profile":active_profile,
        "current":inventory.current.map(|row|row.spec),"other_programs":other_programs,
        "suggestion":existing_scope.or(lan_scope),"suggestion_source":suggestion_source})
}

/// Read the OS owner when this page opens or its listener target changes.
/// This observes configuration; it never claims that another PC can reach it.
pub(super) async fn status(admin: &Admin) -> Result<Value, String> {
    #[cfg(not(windows))]
    {
        let _ = admin;
        return Err("受信許可の確認はWindowsのHub PCで利用できます。".into());
    }
    #[cfg(windows)]
    {
        let (hub, gateway) = {
            let server = admin.network_server.lock().await;
            let (hub, gateway) = network_admin::firewall_endpoints(&server)?;
            (hub.to_owned(), gateway.to_owned())
        };
        let advertised = admin
            .network_hosting
            .lock()
            .map_err(|_| "公開設定を確認できません。")?
            .configured()?
            .advertise_ip
            .to_string();
        let (local_address, local_ports) = listener_target((&hub, &gateway, &advertised))?;
        let (program, _) = executable()?;
        let directory = std::fs::canonicalize(admin.path.parent().ok_or("保存先がありません。")?)
            .map_err(|_| "保存先を確認できません。")?;
        let name = rule_name(
            &program,
            directory.to_str().ok_or("保存先を確認できません。")?,
        );
        let script = format!(
            "{HELPER}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $names=@(Get-NetFirewallRule -PolicyStore PersistentStore | Where-Object {{ $_.Group -ceq 'moyAI Hub managed inbound v1' }} | Select-Object -First 65 -ExpandProperty Name); if ($names.Count -gt 64) {{ throw 'Too many managed rules' }}; $other=@($names | Where-Object {{ $_ -cne '{name}' }} | ForEach-Object {{ Get-MoyaiRule $_ }}); [pscustomobject]@{{current=(Get-MoyaiRule '{name}');other=$other}} | ConvertTo-Json -Depth 8 -Compress"
        );
        let result = run_script(&script, 15).await?;
        if !result.status.success() || result.stdout.len() > 256 * 1024 {
            return Err("受信許可を読み取れません。PC管理者へ確認してください。".into());
        }
        let inventory: Inventory = serde_json::from_slice(&result.stdout)
            .map_err(|_| "受信許可の内容を読み取れません。PC管理者へ確認してください。")?;
        let candidate = hosting_options::inspect()
            .await
            .ok()
            .and_then(|rows| rows.into_iter().find(|row| row.ip == local_address));
        let lan_scope =
            candidate
                .as_ref()
                .and_then(|row| row.trusted_scope())
                .map(|(remote, profile)| Scope {
                    remote_addresses: vec![remote],
                    profiles: vec![profile],
                });
        Ok(status_value(
            &program,
            local_address,
            &local_ports,
            inventory,
            lan_scope,
            candidate.as_ref().and_then(|row| row.profile.as_deref()),
        ))
    }
}

pub(super) async fn preview(admin: &Admin, scope: Scope) -> Result<Value, String> {
    let server = admin.network_server.lock().await;
    let (hub, gateway) = network_admin::firewall_endpoints(&server)?;
    let advertised = admin
        .network_hosting
        .lock()
        .map_err(|_| "公開設定を確認できません。")?
        .configured()?
        .advertise_ip
        .to_string();
    output(prepare(admin, scope, (hub, gateway, &advertised)).await?)
}

fn verify_review(current: &Preview, expected: &str) -> Result<(), String> {
    if current.conflict || current.fingerprint != expected {
        return Err(
            "接続先・実行ファイル・ルールが変わりました。変更内容を再確認してください。".into(),
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct ExportTarget {
    machine_guid: String,
    data_directory: String,
    hub_id: String,
    hosting_bind: String,
    saved_gateway_port: u16,
    gateway_port: u16,
}

fn verify_export_gateway(saved_port: u16, running_port: u16) -> Result<(), String> {
    if saved_port == 0 {
        return Err(
            "Gatewayのポートが自動選択（0）のため、管理者用スクリプトは出力できません。Hub管理画面でGatewayに固定ポートを設定し、端末ネットワークを再起動してから出力してください。現在の起動中ポートへの受信許可は、この画面から直接設定できます。".into(),
        );
    }
    if saved_port != running_port {
        return Err(
            "保存済みGatewayポートと稼働中のポートが異なります。端末ネットワークを再起動し、受信許可の内容を再確認してからスクリプトを出力してください。".into(),
        );
    }
    Ok(())
}

fn export_script(current: &Preview, target: ExportTarget) -> Result<Value, String> {
    verify_export_gateway(target.saved_gateway_port, target.gateway_port)?;
    let mut plan = serde_json::to_value(target).map_err(|_| "設定内容を作成できません。")?;
    plan["request"] = json!({"action":"apply", "spec":current.spec,
        "previous":current.previous,"binary_sha256":current.binary_sha256});
    let plan = serde_json::to_string_pretty(&plan).map_err(|_| "設定内容を作成できません。")?;
    // JSON string escaping prevents path values from terminating this here-string.
    // A BOM keeps Windows PowerShell 5.1 file execution independent of ANSI locale.
    let content = format!(
        "\u{feff}# Run on the original Hub PC in Administrator PowerShell.\n# Add -CheckOnly to inspect without changing Windows.\nparam([switch]$CheckOnly)\n{HELPER}\n{EXPORT_HELPER}\n$export = @'\n{plan}\n'@ | ConvertFrom-Json\nInvoke-MoyaiFirewallExport $export -CheckOnly:$CheckOnly\n"
    );
    Ok(json!({"filename":"Allow-moyAI-Hub.ps1","content":content}))
}

/// Export only the freshly reviewed target; downloading never changes the OS.
pub(super) async fn export(admin: &Admin, scope: Scope, expected: String) -> Result<Value, String> {
    let server = admin.network_server.lock().await;
    let (hub, gateway) = network_admin::firewall_endpoints(&server)?;
    let hosting = admin
        .network_hosting
        .lock()
        .map_err(|_| "公開設定を確認できません。")?
        .configured()?;
    let current = prepare(
        admin,
        scope,
        (hub, gateway, &hosting.advertise_ip.to_string()),
    )
    .await?;
    verify_review(&current, &expected)?;
    let (hub_id, saved_gateway_port) = {
        let network = admin.network.lock().map_err(|_| "端末設定を開けません。")?;
        (network.hub_id().to_owned(), network.gateway_port())
    };
    let gateway_port = reqwest::Url::parse(gateway)
        .ok()
        .and_then(|url| url.port_or_known_default())
        .ok_or("Gatewayのポートを確認できません。")?;
    verify_export_gateway(saved_gateway_port, gateway_port)?;
    #[cfg(windows)]
    let machine_guid = {
        let script = format!(
            "{EXPORT_HELPER}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Get-MoyaiMachineGuid | ConvertTo-Json -Compress"
        );
        let result = run_script(&script, 15).await?;
        if !result.status.success() || result.stdout.len() > 256 {
            return Err("Hub PCの識別情報を確認できません。".into());
        }
        let value: String = serde_json::from_slice(&result.stdout)
            .map_err(|_| "Hub PCの識別情報を読み取れません。")?;
        if value.len() != 36
            || !value.bytes().enumerate().all(|(i, byte)| {
                if [8, 13, 18, 23].contains(&i) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return Err("Hub PCの識別情報を確認できません。".into());
        }
        value.to_ascii_lowercase()
    };
    #[cfg(not(windows))]
    let machine_guid = String::new();
    let directory = std::fs::canonicalize(admin.path.parent().ok_or("保存先がありません。")?)
        .map_err(|_| "保存先を確認できません。")?;
    let target = ExportTarget {
        machine_guid,
        data_directory: directory
            .to_str()
            .ok_or("保存先を確認できません。")?
            .trim_start_matches("\\\\?\\")
            .to_owned(),
        hub_id,
        hosting_bind: hosting.bind.to_string(),
        saved_gateway_port,
        gateway_port,
    };
    export_script(&current, target)
}

pub(super) async fn change(
    admin: &Admin,
    scope: Scope,
    expected: String,
    remove: bool,
) -> Result<Value, String> {
    // Keep this exact pair of listeners stable through the bounded local approval.
    // The elevated helper expires before this lock can be released on timeout.
    let server = admin.network_server.lock().await;
    let (hub, gateway) = network_admin::firewall_endpoints(&server)?;
    let advertised = admin
        .network_hosting
        .lock()
        .map_err(|_| "公開設定を確認できません。")?
        .configured()?
        .advertise_ip
        .to_string();
    let current = prepare(admin, scope, (hub, gateway, &advertised)).await?;
    verify_review(&current, &expected)?;
    if (remove && current.previous.is_none()) || (!remove && current.matching) {
        return Ok(json!({"status":"unchanged","preview":current}));
    }
    let request = json!({"action":if remove {"remove"} else {"apply"},"spec":current.spec,"previous":current.previous,"binary_sha256":current.binary_sha256,"expires_at_ms":now_ms()+60_000});
    let body = payload(&request)?;
    let child_script = format!(
        "{HELPER}\ntry {{ $request=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{body}')) | ConvertFrom-Json; Invoke-MoyaiFirewall $request; exit 0 }} catch {{ exit 22 }}"
    );
    let encoded = encoded_script(&child_script);
    if encoded.len() > 30_000 {
        return Err("設定内容が上限を超えています。接続元の範囲を整理してください。".into());
    }
    #[cfg(windows)]
    let outcome = {
        // The child command is a fixed embedded asset plus typed base64 JSON.
        // No temporary elevated script/result file or web-supplied path is used.
        let launcher = elevation_script(&encoded);
        match run_script(&launcher, 90).await {
            Ok(result) => result.status.code(),
            Err(_) => None,
        }
    };
    #[cfg(not(windows))]
    let outcome: Option<i32> = None;
    let observed = observe(&current.spec.name).await;
    match observed {
        Ok(observed) => {
            let after = preview_value(current.spec, current.binary_sha256, observed)?;
            let verified = if remove {
                after.previous.is_none()
            } else {
                after.matching && !after.conflict
            };
            let status = if outcome == Some(0) && verified {
                "verified"
            } else if outcome == Some(20) {
                "cancelled"
            } else {
                "unconfirmed"
            };
            Ok(json!({"status":status,"preview":after}))
        }
        Err(_) => Ok(json!({"status":"unconfirmed","preview":null})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_gateway_requires_a_stable_saved_port_that_matches_the_running_listener() {
        assert!(verify_export_gateway(9472, 9472).is_ok());
        // A legacy dynamic profile must give an actionable export error both
        // immediately and after a restart assigns another ephemeral port.
        for running in [51001, 51002] {
            assert!(
                verify_export_gateway(0, running)
                    .unwrap_err()
                    .contains("固定ポート")
            );
        }
        assert!(
            verify_export_gateway(9472, 51001)
                .unwrap_err()
                .contains("再起動")
        );
        assert!(verify_export_gateway(9472, 0).is_err());
    }
    #[test]
    fn exported_review_rejects_any_changed_target_or_conflicting_os_rule() {
        let current = preview_value(spec(), "a".repeat(64), None).unwrap();
        assert!(verify_review(&current, &current.fingerprint).is_ok());
        assert!(verify_review(&current, "obsolete").is_err());
        let replaced = preview_value(spec(), "b".repeat(64), None).unwrap();
        assert!(verify_review(&replaced, &current.fingerprint).is_err());
        let conflict = preview_value(
            spec(),
            "a".repeat(64),
            Some(Observed {
                spec: spec(),
                owned: false,
            }),
        )
        .unwrap();
        assert!(verify_review(&conflict, &conflict.fingerprint).is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn exported_file_parses_and_checks_exact_local_target_without_admin_or_os_writes() {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-firewall-export-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("firewall's-export-")
            .tempdir_in(parent)
            .unwrap();
        let program = directory.path().join("hub's.exe");
        std::fs::write(&program, b"inert test executable bytes").unwrap();
        std::fs::write(
            directory.path().join("network-hosting.json"),
            br#"{"schema_version":1,"bind":"0.0.0.0:9471","advertise_ip":"192.168.1.2"}"#,
        )
        .unwrap();
        std::fs::write(
            directory.path().join("device-network.json"),
            br#"{"schema_version":5,"hub_id":"fixture-hub","gateway_port":9472}"#,
        )
        .unwrap();
        let machine = run_script(
            &format!("{EXPORT_HELPER}\nGet-MoyaiMachineGuid | ConvertTo-Json -Compress"),
            15,
        )
        .await
        .unwrap();
        assert!(machine.status.success());
        let mut s = spec();
        s.program = program.to_str().unwrap().to_owned();
        let current = preview_value(s, digest(b"inert test executable bytes"), None).unwrap();
        let exported = export_script(
            &current,
            ExportTarget {
                machine_guid: serde_json::from_slice(&machine.stdout).unwrap(),
                data_directory: directory.path().to_str().unwrap().to_owned(),
                hub_id: "fixture-hub".into(),
                hosting_bind: "0.0.0.0:9471".into(),
                saved_gateway_port: 9472,
                gateway_port: 9472,
            },
        )
        .unwrap();
        assert_eq!(exported["filename"], "Allow-moyAI-Hub.ps1");
        let script_path = directory.path().join("Allow-moyAI-Hub.ps1");
        let content = exported["content"].as_str().unwrap();
        assert!(content.starts_with('\u{feff}'));
        std::fs::write(&script_path, content).unwrap();
        let foreign_module = directory
            .path()
            .join("modules/Microsoft.PowerShell.Utility");
        std::fs::create_dir_all(&foreign_module).unwrap();
        std::fs::write(
            foreign_module.join("Microsoft.PowerShell.Utility.psd1"),
            "@{ ModuleVersion='99.0'; FunctionsToExport=@(); CmdletsToExport=@() }",
        )
        .unwrap();
        // Every NetSecurity entry available to the exported check is replaced.
        // No real host rule is read, created, changed or removed by this fixture.
        // A foreign Utility module simulates PS7's inherited PSModulePath, where
        // a successful name-only import omits Windows PS5.1's Get-FileHash.
        let check = format!(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $env:PSModulePath='{}'; Import-Module Microsoft.PowerShell.Utility; function Get-NetFirewallRule {{ [CmdletBinding()]param($Name,$PolicyStore) }}; function New-NetFirewallRule {{ throw 'OS mutation forbidden' }}; function Set-NetFirewallRule {{ throw 'OS mutation forbidden' }}; function Remove-NetFirewallRule {{ throw 'OS mutation forbidden' }}; try {{ & '{}' -CheckOnly; exit 0 }} catch {{ [Console]::WriteLine($_.ToString()); [Console]::WriteLine($_.ScriptStackTrace); exit 22 }}",
            foreign_module
                .parent()
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\'', "''"),
            script_path.to_str().unwrap().replace('\'', "''")
        );
        let success = run_script(&check, 15).await.unwrap();
        assert!(
            success.status.success(),
            "{}",
            String::from_utf8_lossy(&success.stdout)
        );
        assert!(
            String::from_utf8_lossy(&success.stdout)
                .contains("Checks passed. No firewall settings were changed.")
        );
        std::fs::write(&program, b"different build").unwrap();
        assert_eq!(
            run_script(&check, 15).await.unwrap().status.code(),
            Some(22)
        );
    }

    #[test]
    fn listener_target_consumes_the_running_https_projections_and_never_loopback_or_other_hosts() {
        assert_eq!(
            listener_target((
                "https://192.168.1.2:9471",
                "https://192.168.1.2:9472",
                "192.168.1.2"
            ))
            .unwrap(),
            ("192.168.1.2".parse().unwrap(), vec![9471, 9472])
        );
        assert!(
            listener_target((
                "https://127.0.0.1:9471",
                "https://127.0.0.1:9472",
                "127.0.0.1"
            ))
            .is_err()
        );
        assert!(
            listener_target((
                "https://192.168.1.2:9471",
                "https://192.168.1.3:9472",
                "192.168.1.2"
            ))
            .is_err()
        );
        assert!(
            listener_target((
                "http://192.168.1.2:9471",
                "https://192.168.1.2:9472",
                "192.168.1.2"
            ))
            .is_err()
        );
        assert!(
            listener_target((
                "https://192.168.1.2:9471",
                "https://192.168.1.2:9471",
                "192.168.1.2"
            ))
            .is_err()
        );
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_powershell_executes_embedded_functions_and_query_through_the_real_transport() {
        // Override the only OS query reached by this absent-rule fixture.
        // No NetSecurity module or host Firewall state is accessed.
        let script = format!(
            "{HELPER}\nfunction Get-NetFirewallRule {{ [CmdletBinding()] param($PolicyStore,$Name) return }}\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $r=Get-MoyaiRule 'moyai-hub-inbound-v1-aaaaaaaaaaaaaaaaaaaaaaaa'; @{{reached='末尾まで実行';absent=($null -eq $r)}} | ConvertTo-Json -Compress"
        );
        let result = run_script(&script, 15).await.unwrap();
        assert!(result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["reached"], "末尾まで実行");
        assert_eq!(value["absent"], true);
        let script = format!(
            "function Get-NetFirewallRule {{ [CmdletBinding()] param($PolicyStore,$Name) return }}\n{}",
            observation_script("moyai-hub-inbound-v1-aaaaaaaaaaaaaaaaaaaaaaaa")
        );
        let absent = run_script(&script, 15).await.unwrap();
        assert!(absent.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&absent.stdout).unwrap(),
            json!({"observed":null})
        );
        assert!(decode_observation(&absent.stdout).unwrap().is_none());
        assert!(decode_observation(b" \r\n").is_err());
        assert!(decode_observation(b"{}").is_err());
        assert!(decode_observation(b"null").is_err());
        assert_eq!(
            run_script("exit 22", 15).await.unwrap().status.code(),
            Some(22)
        );
        for (code, expected) in [(1223, 20), (5, 23)] {
            let mock = format!(
                "function Start-Process {{ throw [InvalidOperationException]::new('launch failed',[ComponentModel.Win32Exception]::new({code})) }}\n{}",
                elevation_script("not-executed")
            );
            assert_eq!(
                run_script(&mock, 15).await.unwrap().status.code(),
                Some(expected)
            );
        }
    }
    #[test]
    fn scopes_are_explicit_private_and_canonical() {
        for invalid in [
            "Any",
            "0.0.0.0/0",
            "192.168.0.0/8",
            "172.0.0.0/8",
            "127.0.0.1",
            "8.8.8.8",
            "192.168.1.4/24",
            "192.168.1.1;whoami",
        ] {
            assert!(subnet(invalid).is_err(), "{invalid}");
        }
        assert_eq!(subnet("192.168.1.4").unwrap(), "192.168.1.4/32");
        for valid in ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"] {
            assert_eq!(subnet(valid).unwrap(), valid);
        }
        assert!(
            normalize(Scope {
                remote_addresses: vec!["10.1.1.1".into()],
                profiles: vec!["Public".into()]
            })
            .is_err()
        );
        assert!(
            normalize(Scope {
                remote_addresses: vec![],
                profiles: vec!["Private".into()]
            })
            .is_err()
        );
    }
    fn spec() -> RuleSpec {
        RuleSpec {
            name: rule_name("C:\\Hub\\moyai-hub.exe", "C:\\Data"),
            program: "C:\\Hub\\moyai-hub.exe".into(),
            local_address: "192.168.1.2".parse().unwrap(),
            local_ports: vec![9471, 9472],
            remote_addresses: vec!["192.168.1.0/24".into()],
            profile_mask: 2,
        }
    }
    #[test]
    fn preview_tracks_exact_rule_executable_and_scopes_without_an_extra_receipt() {
        let s = spec();
        let a = preview_value(s.clone(), "old".into(), None).unwrap();
        let matching = preview_value(
            s.clone(),
            "old".into(),
            Some(Observed {
                spec: s.clone(),
                owned: true,
            }),
        )
        .unwrap();
        assert!(matching.matching);
        assert!(!matching.conflict);
        assert_ne!(a.fingerprint, matching.fingerprint);
        let changed = preview_value(
            s.clone(),
            "new".into(),
            matching
                .previous
                .clone()
                .map(|spec| Observed { spec, owned: true }),
        )
        .unwrap();
        assert_ne!(matching.fingerprint, changed.fingerprint);
        let conflict = preview_value(
            s.clone(),
            "old".into(),
            Some(Observed {
                spec: s.clone(),
                owned: false,
            }),
        )
        .unwrap();
        assert!(conflict.conflict);
        let mut other = s.clone();
        other.program = "C:\\Other\\moyai-hub.exe".into();
        assert!(
            preview_value(
                s,
                "old".into(),
                Some(Observed {
                    spec: other,
                    owned: true
                })
            )
            .unwrap()
            .conflict
        );
    }
    #[test]
    fn rule_identity_separates_installations_and_profiles() {
        assert_eq!(
            rule_name("C:\\Hub.exe", "C:\\Data"),
            rule_name("c:\\hub.exe", "\\\\?\\C:\\Data")
        );
        assert_ne!(
            rule_name("C:\\Hub.exe", "C:\\Data"),
            rule_name("C:\\Other.exe", "C:\\Data")
        );
        assert_ne!(
            rule_name("C:\\Hub.exe", "C:\\Data"),
            rule_name("C:\\Hub.exe", "C:\\Other")
        );
    }
    #[test]
    fn current_rule_status_detects_relocation_and_port_changes_without_inheriting_scope() {
        let s = spec();
        let mut previous = s.clone();
        previous.program = "C:\\Old\\moyai-hub.exe".into();
        let value = status_value(
            &s.program,
            s.local_address,
            &s.local_ports,
            Inventory {
                current: None,
                other: vec![Observed {
                    spec: previous,
                    owned: true,
                }],
            },
            None,
            Some("Private"),
        );
        assert_eq!(value["state"], "missing");
        assert_eq!(value["other_programs"].as_array().unwrap().len(), 1);
        assert!(value["suggestion"].is_null());
        let value = status_value(
            &s.program,
            s.local_address,
            &[9471, 9999],
            Inventory {
                current: Some(Observed {
                    spec: s.clone(),
                    owned: true,
                }),
                other: vec![],
            },
            None,
            Some("Private"),
        );
        assert_eq!(value["state"], "mismatch");
        assert_eq!(
            value["suggestion"]["remote_addresses"],
            json!(["192.168.1.0/24"])
        );
        assert_eq!(value["suggestion_source"], "existing");
        let value = status_value(
            &s.program,
            s.local_address,
            &s.local_ports,
            Inventory {
                current: Some(Observed {
                    spec: s.clone(),
                    owned: true,
                }),
                other: vec![],
            },
            None,
            Some("Private"),
        );
        assert_eq!(value["state"], "matching");
        let public_network = status_value(
            &s.program,
            s.local_address,
            &s.local_ports,
            Inventory {
                current: Some(Observed {
                    spec: s.clone(),
                    owned: true,
                }),
                other: vec![],
            },
            None,
            Some("Public"),
        );
        assert_eq!(public_network["state"], "mismatch");
        let value = status_value(
            &s.program,
            s.local_address,
            &s.local_ports,
            Inventory {
                current: Some(Observed {
                    spec: s.clone(),
                    owned: false,
                }),
                other: vec![],
            },
            None,
            Some("Private"),
        );
        assert_eq!(value["state"], "conflict");
        assert!(value["suggestion"].is_null());
    }
}
