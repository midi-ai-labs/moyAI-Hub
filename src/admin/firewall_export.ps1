# The exported script reuses the exact rule validator and writer embedded by Hub.
function Get-MoyaiMachineGuid {
    $value=[Microsoft.Win32.Registry]::GetValue('HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography','MachineGuid',$null)
    return ([Guid]::Parse([string]$value)).ToString()
}
function Read-MoyaiExportSettings([string]$Path,[long]$MaximumBytes) {
    $stream=[IO.File]::OpenRead($Path)
    try {
        if ($stream.Length -gt $MaximumBytes) { throw 'Hub settings exceed the supported size' }
        $reader=[IO.StreamReader]::new($stream,[Text.Encoding]::UTF8,$true)
        try { return ($reader.ReadToEnd() | ConvertFrom-Json) } finally { $reader.Dispose() }
    } finally { $stream.Dispose() }
}
function Assert-MoyaiExportTarget($Export) {
    if ($Export.saved_gateway_port -eq 0 -or $Export.saved_gateway_port -ne $Export.gateway_port -or $Export.request.spec.local_ports -notcontains $Export.gateway_port) {
        throw 'A fixed Gateway port matching the reviewed listener is required. Set a fixed port in Hub, restart its device network, and export again.'
    }
    if ((Get-MoyaiMachineGuid) -cne $Export.machine_guid) { throw 'This script belongs to another Hub PC. Export it again on this PC.' }
    $hosting=Read-MoyaiExportSettings (Join-Path $Export.data_directory 'network-hosting.json') 16384
    $network=Read-MoyaiExportSettings (Join-Path $Export.data_directory 'device-network.json') 33554432
    if ($hosting.schema_version -ne 1 -or $hosting.bind -cne $Export.hosting_bind -or $hosting.advertise_ip -cne $Export.request.spec.local_address -or $network.schema_version -ne 5 -or $network.hub_id -cne $Export.hub_id -or $network.gateway_port -ne $Export.saved_gateway_port) {
        throw 'Hub identity or connection settings changed. Export a new script from Hub.'
    }
}
function Test-MoyaiAdministrator {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    try { return ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) } finally { $identity.Dispose() }
}
function Invoke-MoyaiFirewallExport($Export,[switch]$CheckOnly) {
    Assert-MoyaiExportTarget $Export
    $request=$Export.request
    $request | Add-Member -NotePropertyName expires_at_ms -NotePropertyValue ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+60000) -Force
    $null=Assert-MoyaiFirewallRequest $request
    Write-Host 'moyAI Hub: the following inbound TCP rule was reviewed in Hub.'
    Write-Host ($request.spec | ConvertTo-Json -Depth 8)
    Write-Host ('Executable SHA256: '+$request.binary_sha256)
    if ($CheckOnly) { Write-Output 'Checks passed. No firewall settings were changed.'; return }
    if (-not (Test-MoyaiAdministrator)) { throw 'Open PowerShell as administrator on this Hub PC, then run this script again. Use -CheckOnly to inspect without changes.' }
    if ((Read-Host 'Type APPLY to permit only the connections listed above') -cne 'APPLY') { Write-Output 'Cancelled. No firewall settings were changed.'; return }
    # Recheck after the administrator has reviewed the displayed target.
    Assert-MoyaiExportTarget $Export
    $request.expires_at_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+60000
    Invoke-MoyaiFirewall $request
    Write-Output 'Windows inbound permission verified. Reopen Hub and test the connection from the other PC.'
}
