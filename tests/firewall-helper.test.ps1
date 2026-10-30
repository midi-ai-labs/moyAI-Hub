# Isolated NetSecurity fakes: this test never reads or changes the host firewall.
$ErrorActionPreference='Stop'
# Load built-in utility commands before installing fakes. Windows PowerShell 5.1
# otherwise imports its Get-FileHash function lazily over the test double.
Import-Module Microsoft.PowerShell.Utility
. (Join-Path $PSScriptRoot '../src/admin/firewall_helper.ps1')
$script:rule=$null; $script:mutations=0; $script:partial=$false; $script:package='Any'
function Get-NetFirewallRule { [CmdletBinding()] param($Name,$PolicyStore) if ($PolicyStore -ne 'PersistentStore') { throw 'Unexpected store' }; if ($script:rule -and $script:rule.Name -ceq $Name) { $script:rule } }
function Get-NetFirewallApplicationFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{Program=$script:rule.Program;Package=$script:package} } }
function Get-NetFirewallAddressFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{LocalAddress=$script:rule.LocalAddress;RemoteAddress=$script:rule.RemoteAddress} } }
function Get-NetFirewallPortFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{Protocol=$script:rule.Protocol;LocalPort=$script:rule.LocalPort;RemotePort='Any'} } }
function Get-NetFirewallServiceFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{Service='Any'} } }
function Get-NetFirewallInterfaceFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{InterfaceAlias='Any'} } }
function Get-NetFirewallInterfaceTypeFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{InterfaceType='Any'} } }
function Get-NetFirewallSecurityFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{Authentication='NotRequired';Encryption='NotRequired';OverrideBlockRules='False';LocalUser='Any';RemoteUser='Any';RemoteMachine='Any'} } }
function Get-FileHash { param($LiteralPath,$Algorithm) if ($LiteralPath -cne 'C:\Hub\moyai-hub.exe' -or $Algorithm -ne 'SHA256') { throw 'Unexpected file' }; [pscustomobject]@{Hash=('a'*64)} }
function New-NetFirewallRule {
    [CmdletBinding()] param($PolicyStore,$Name,$DisplayName,$Description,$Group,$Enabled,$Direction,$Action,$EdgeTraversalPolicy,$Profile,$Program,$Protocol,$LocalAddress,$LocalPort,$RemoteAddress,$RemotePort,$InterfaceType,$Authentication,$Encryption,$OverrideBlockRules,$LooseSourceMapping,$LocalOnlyMapping)
    if ($script:rule -or $PolicyStore -ne 'PersistentStore') { throw 'Existing or wrong store' }
    $script:mutations++
    $script:rule=[pscustomobject]@{Name=$Name;DisplayName=$DisplayName;Description=$Description;Group=$Group;Enabled=$Enabled;Direction=$Direction;Action=$Action;EdgeTraversalPolicy=$EdgeTraversalPolicy;Profile=$Profile;Program=$Program;Protocol=$Protocol;LocalAddress=$LocalAddress;LocalPort=$LocalPort;RemoteAddress=$RemoteAddress;LooseSourceMapping=$LooseSourceMapping;LocalOnlyMapping=$LocalOnlyMapping}
}
function Set-NetFirewallRule {
    [CmdletBinding()] param($PolicyStore,$Name,$Description,$Enabled,$Direction,$Action,$EdgeTraversalPolicy,$Profile,$Program,$Protocol,$LocalAddress,$LocalPort,$RemoteAddress,$RemotePort,$InterfaceType,$Authentication,$Encryption,$OverrideBlockRules,$LooseSourceMapping,$LocalOnlyMapping)
    if (-not $script:rule -or $script:rule.Name -cne $Name -or $PolicyStore -ne 'PersistentStore') { throw 'Wrong update target' }
    $script:mutations++
    foreach($key in @('Description','Enabled','Direction','Action','EdgeTraversalPolicy','Profile','Program','Protocol','LocalAddress','LocalPort','RemoteAddress','LooseSourceMapping','LocalOnlyMapping')) { $script:rule.$key=$PSBoundParameters[$key] }
    if ($script:partial) { $script:rule.RemoteAddress=@('10.9.9.9/32'); throw 'Partial OS write' }
}
function Remove-NetFirewallRule { [CmdletBinding(SupportsShouldProcess)] param($PolicyStore,$Name) if ($PolicyStore -ne 'PersistentStore' -or $script:rule.Name -cne $Name) { throw 'Wrong removal target' }; $script:mutations++; $script:rule=$null }
$script:checks=0
function Assert($Value,[string]$Message) { $script:checks++; if (-not $Value) { throw $Message } }
function Reject([scriptblock]$Action,[string]$Message) { $before=$script:mutations; $caught=$false; try { & $Action } catch { $caught=$true }; Assert $caught $Message; Assert ($script:mutations -eq $before) 'Rejected action mutated a rule' }
function Spec { [pscustomobject]@{name=('moyai-hub-inbound-v1-'+('b'*24));program='C:\Hub\moyai-hub.exe';local_address='192.168.1.2';local_ports=@(9471,9472);remote_addresses=@('192.168.1.0/24');profile_mask=2} }
function Request($Spec,$Previous=$null,$Action='apply') { [pscustomobject]@{spec=$Spec;previous=$Previous;action=$Action;binary_sha256=('a'*64);expires_at_ms=([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+55000)} }
$spec=Spec
Invoke-MoyaiFirewall (Request $spec)
Assert ($script:mutations -eq 1) 'First apply creates one combined-port rule'
$row=Get-MoyaiRule $spec.name
Assert $row.owned 'Created filters are owned'
Assert (Test-MoyaiSame $row.spec $spec) 'Created scope matches'
Invoke-MoyaiFirewall (Request $spec $row.spec)
Assert ($script:mutations -eq 1) 'Exact repeat is idempotent'
# Windows can return dotted masks and a null Package for the same desktop rule.
# Observe those representations without rewriting the OS rule or its owner hash.
$script:rule.RemoteAddress=@('192.168.1.0/255.255.255.0')
Assert ((Get-MoyaiRule $spec.name).owned) 'Dotted subnet mask preserves managed ownership'
$script:package=$null
Assert ((Get-MoyaiRule $spec.name).owned) 'No package filter preserves desktop rule ownership'
Invoke-MoyaiFirewall (Request $spec $row.spec)
Assert ($script:mutations -eq 1) 'Equivalent Windows representations do not rewrite a rule'
foreach ($package in @('', ' ', 'S-1-15-2-12345')) {
    $script:package=$package
    Assert (-not (Get-MoyaiRule $spec.name).owned) 'A different package filter is not owned'
    Reject { Invoke-MoyaiFirewall (Request $spec $row.spec) } 'Package-scoped rules cannot be overwritten'
    Reject { Invoke-MoyaiFirewall (Request $spec $row.spec 'remove') } 'Package-scoped rules cannot be removed'
}
$script:package=$null
foreach ($remote in @('192.168.1.0/255.255.0.255','192.168.1.0/255.255.255.256','192.168.1.0/255.255.255.00','192.168.1.2/255.255.255.0','192.168.1.0/255.255.255.128','0.0.0.0/0.0.0.0','8.8.8.8/255.255.255.255')) {
    $script:rule.RemoteAddress=@($remote)
    Assert (-not (Get-MoyaiRule $spec.name).owned) "Invalid or changed observed scope $remote must remain foreign"
    Reject { Invoke-MoyaiFirewall (Request $spec $row.spec) } 'Changed scope cannot be overwritten'
    Reject { Invoke-MoyaiFirewall (Request $spec $row.spec 'remove') } 'Changed scope cannot be removed'
}
$script:rule.RemoteAddress=$spec.remote_addresses
# Normalization belongs only to OS observation, not caller-supplied requests.
$dotted=Spec; $dotted.remote_addresses=@('192.168.1.0/255.255.255.0')
Reject { Invoke-MoyaiFirewall (Request $dotted $row.spec) } 'Requests still require canonical CIDR'
foreach ($scopes in @(
    @{cidr=@('10.0.0.0/8');os=@('10.0.0.0/255.0.0.0')},
    @{cidr=@('172.16.0.0/12');os=@('172.16.0.0/255.240.0.0')},
    @{cidr=@('192.168.0.0/16');os=@('192.168.0.0/255.255.0.0')},
    @{cidr=@('192.168.1.128/25');os=@('192.168.1.128/255.255.255.128')},
    @{cidr=@('10.1.2.3/32','192.168.1.0/24');os=@('192.168.1.0/255.255.255.0','10.1.2.3')},
    @{cidr=@('10.1.2.3/32');os=@('10.1.2.3/255.255.255.255')}
)) {
    $equivalent=Spec; $equivalent.remote_addresses=$scopes.cidr
    $script:rule.Description=Get-MoyaiDescription $equivalent
    $script:rule.RemoteAddress=$scopes.os
    $observed=Get-MoyaiRule $spec.name
    Assert $observed.owned 'Valid Windows scope representation is owned'
    Assert (Test-MoyaiSame $observed.spec $equivalent) 'Observed scope equals canonical request'
    Invoke-MoyaiFirewall (Request $equivalent $equivalent)
    Assert ($script:mutations -eq 1) 'Equivalent scope comparison is read-only'
}
$script:rule.Description=Get-MoyaiDescription $spec
$script:rule.RemoteAddress=$spec.remote_addresses
$script:package='Any'
$updated=Spec; $updated.remote_addresses=@('10.1.2.3/32'); $updated.profile_mask=1
Invoke-MoyaiFirewall (Request $updated $row.spec)
Assert ($script:mutations -eq 2) 'Scoped update applies once'
Assert ((Get-MoyaiRule $spec.name).spec.profile_mask -eq 1) 'Updated profile is exact'
Reject { Invoke-MoyaiFirewall (Request $spec $row.spec) } 'Stale prior rule must be rejected'
$current=(Get-MoyaiRule $spec.name).spec
foreach ($flag in @('LooseSourceMapping','LocalOnlyMapping')) {
    $script:rule.$flag=$true
    Assert (-not (Get-MoyaiRule $spec.name).owned) "Changed $flag must be foreign"
    Reject { Invoke-MoyaiFirewall (Request $spec $current 'remove') } "Changed $flag cannot be removed"
    $script:rule.$flag=$false
}
$script:rule.RemoteAddress=@('10.2.3.4/32')
Assert (-not (Get-MoyaiRule $spec.name).owned) 'Manual change invalidates owner metadata'
Reject { Invoke-MoyaiFirewall (Request $spec $current) } 'Manual changes cannot be overwritten'
Reject { Invoke-MoyaiFirewall (Request $spec $current 'remove') } 'Manual changes cannot be removed'
$script:rule.RemoteAddress=$current.remote_addresses
foreach ($remote in @('Any','0.0.0.0/0','8.8.8.8/32','192.168.1.2/24','172.0.0.0/8','10.0.0.0/1',"10.0.0.1/32';whoami")) { $bad=Spec; $bad.remote_addresses=@($remote); Reject { Invoke-MoyaiFirewall (Request $bad $current) } "Invalid subnet $remote" }
$bad=Spec; $bad.profile_mask=4; Reject { Invoke-MoyaiFirewall (Request $bad $current) } 'Public profile must fail'
$bad=Spec; $bad.local_ports=@(9471,0); Reject { Invoke-MoyaiFirewall (Request $bad $current) } 'Zero port must fail'
$bad=Spec; $bad.local_address='0.0.0.0'; Reject { Invoke-MoyaiFirewall (Request $bad $current) } 'Any local address must fail'
$bad=Request $spec $current; $bad.binary_sha256='c'*64; Reject { Invoke-MoyaiFirewall $bad } 'Changed executable must fail'
$bad=Request $spec $current; $bad.expires_at_ms=1; Reject { Invoke-MoyaiFirewall $bad } 'Expired UAC request must fail'
$bad=Request $spec $current; $bad.expires_at_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+3600000; Reject { Invoke-MoyaiFirewall $bad } 'Unbounded approval must fail'
$other=Spec; $other.program='C:\Other\moyai-hub.exe'; Reject { Invoke-MoyaiFirewall (Request $spec $other) } 'Other installation must fail'
$script:partial=$true; $failed=$false
try { Invoke-MoyaiFirewall (Request $spec $current) } catch { $failed=$true }
Assert $failed 'Partial OS failure must fail'
Assert (-not (Get-MoyaiRule $spec.name).owned) 'Partial state remains visible as conflict'
$script:partial=$false; $script:rule=$null
Invoke-MoyaiFirewall (Request $spec)
$current=(Get-MoyaiRule $spec.name).spec
Invoke-MoyaiFirewall (Request $spec $current 'remove')
Assert ($null -eq (Get-MoyaiRule $spec.name)) 'Exact removal verified'
$before=$script:mutations; Invoke-MoyaiFirewall (Request $spec $null 'remove')
Assert ($before -eq $script:mutations) 'Repeated absence removal is idempotent'
# The reviewer is the new executable. The removed old executable need not exist.
Invoke-MoyaiFirewall (Request $spec)
$old=Spec; $old.program='C:\Deleted\moyai-hub.exe'
$profile='C:\OldData'
$sha=[Security.Cryptography.SHA256]::Create()
try { $old.name='moyai-hub-inbound-v1-'+([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($old.program.ToLowerInvariant()+"`n"+$profile.ToLowerInvariant())))).Replace('-','').ToLowerInvariant().Substring(0,24) } finally { $sha.Dispose() }
$script:rule.Program=$old.program; $script:rule.Name=$old.name
$script:rule.DisplayName='moyAI Hub TCP '+$old.name.Substring(21); $script:rule.Description=Get-MoyaiDescription $old
function RetiredRequest { $request=Request $old $old 'remove_retired'; $request | Add-Member profile_directory $profile; $request | Add-Member reviewer_program 'C:\Hub\moyai-hub.exe'; $request }
$bad=RetiredRequest; $bad.profile_directory='C:\OtherProfile'
Reject { Invoke-MoyaiFirewall $bad } 'Retired removal cannot target another profile'
$bad=RetiredRequest; $bad.binary_sha256='b'*64
Reject { Invoke-MoyaiFirewall $bad } 'Retired removal checks the new executable'
$bad=RetiredRequest; $bad.expires_at_ms=1
Reject { Invoke-MoyaiFirewall $bad } 'Retired removal expires'
$script:rule.RemoteAddress=@('10.0.0.1/32')
Reject { Invoke-MoyaiFirewall (RetiredRequest) } 'Retired removal preserves manual edits'
$script:rule.RemoteAddress=$old.remote_addresses
Invoke-MoyaiFirewall (RetiredRequest)
Assert ($null -eq $script:rule) 'Retired removal does not need the deleted executable'
. (Join-Path $PSScriptRoot '../src/admin/firewall_export.ps1')
$script:machine='11111111-1111-1111-1111-111111111111'
$script:hosting=[pscustomobject]@{schema_version=1;bind='0.0.0.0:9471';advertise_ip='192.168.1.2'}
$script:network=[pscustomobject]@{schema_version=5;hub_id='test-hub';gateway_port=9472}
$script:isAdministrator=$true; $script:answer='APPLY'; $script:afterPrompt=$null
function Get-MoyaiMachineGuid { $script:machine }
function Read-MoyaiExportSettings($Path,$MaximumBytes) {
    switch ($Path) {
        'C:\Data\network-hosting.json' { if ($MaximumBytes -ne 16384) { throw 'Wrong hosting bound' }; $script:hosting }
        'C:\Data\device-network.json' { if ($MaximumBytes -ne 33554432) { throw 'Wrong network bound' }; $script:network }
        default { throw 'Unexpected data path' }
    }
}
function Test-MoyaiAdministrator { $script:isAdministrator }
function Read-Host($Prompt) { if ($script:afterPrompt) { & $script:afterPrompt }; $script:answer }
function ExportPlan { [pscustomobject]@{machine_guid='11111111-1111-1111-1111-111111111111';data_directory='C:\Data';hub_id='test-hub';hosting_bind='0.0.0.0:9471';saved_gateway_port=9472;gateway_port=9472;request=(Request (Spec))} }
$plan=ExportPlan
$script:isAdministrator=$false
$before=$script:mutations
Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null
Assert ($before -eq $script:mutations) 'Unprivileged exported check never changes firewall'
Reject { Invoke-MoyaiFirewallExport $plan | Out-Null } 'Exported apply needs administrator'
$script:isAdministrator=$true; $script:answer='cancel'
Invoke-MoyaiFirewallExport $plan | Out-Null
Assert ($before -eq $script:mutations) 'Declined exported confirmation never changes firewall'
$script:answer='APPLY'
$script:machine='22222222-2222-2222-2222-222222222222'
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Exported script rejects another machine'
$script:machine=$plan.machine_guid
$script:hosting.bind='0.0.0.0:9944'
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Exported script rejects changed Hub port'
$script:hosting.bind=$plan.hosting_bind
$script:network.gateway_port=9945
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Exported script rejects changed Gateway port'
$script:network.gateway_port=$plan.gateway_port
$dynamic=ExportPlan; $dynamic.saved_gateway_port=0
Reject { Invoke-MoyaiFirewallExport $dynamic -CheckOnly | Out-Null } 'Legacy dynamic Gateway export requires a fixed port before first use'
$dynamic.gateway_port=51002
Reject { Invoke-MoyaiFirewallExport $dynamic -CheckOnly | Out-Null } 'Restarted dynamic Gateway cannot reuse its earlier exported target'
$stale=ExportPlan; $stale.saved_gateway_port=9945
Reject { Invoke-MoyaiFirewallExport $stale -CheckOnly | Out-Null } 'Saved and running Gateway ports must agree'
$script:network.gateway_port=0
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Fixed export rejects later switch to dynamic Gateway'
$script:network.gateway_port=$plan.saved_gateway_port
$script:network.hub_id='replacement-hub'
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Exported script rejects replaced Hub identity'
$script:network.hub_id=$plan.hub_id
$script:afterPrompt={ $script:hosting.advertise_ip='192.168.1.99' }
Reject { Invoke-MoyaiFirewallExport $plan | Out-Null } 'Exported script rechecks target after confirmation'
$script:hosting.advertise_ip='192.168.1.2'; $script:afterPrompt=$null
$bad=ExportPlan; $bad.request.binary_sha256='c'*64
Reject { Invoke-MoyaiFirewallExport $bad -CheckOnly | Out-Null } 'Exported script rejects changed binary'
Invoke-MoyaiFirewallExport $plan | Out-Null
Assert ($script:mutations -eq $before+1) 'Confirmed exported apply writes exactly one combined rule'
Assert ((Get-MoyaiRule $plan.request.spec.name).owned) 'Exported apply retains exact managed ownership'
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Prior absent export becomes stale after rule creation'
$current=(Get-MoyaiRule $spec.name).spec
$plan.request.previous=$current
Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null
Assert ($script:mutations -eq $before+1) 'Fresh matching exported check is read-only'
$script:rule.Description='Manual edit'
Reject { Invoke-MoyaiFirewallExport $plan -CheckOnly | Out-Null } 'Exported script cannot replace a conflicting rule'
Write-Output "Firewall helper fake checks: $script:checks PASS; host firewall untouched"
