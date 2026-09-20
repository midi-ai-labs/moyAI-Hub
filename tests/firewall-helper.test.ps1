# Isolated NetSecurity fakes: this test never reads or changes the host firewall.
$ErrorActionPreference='Stop'
# Load built-in utility commands before installing fakes. Windows PowerShell 5.1
# otherwise imports its Get-FileHash function lazily over the test double.
Import-Module Microsoft.PowerShell.Utility
$script:rule=$null; $script:mutations=0; $script:partial=$false
function Get-NetFirewallRule { [CmdletBinding()] param($Name,$PolicyStore) if ($PolicyStore -ne 'PersistentStore') { throw 'Unexpected store' }; if ($script:rule -and $script:rule.Name -ceq $Name) { $script:rule } }
function Get-NetFirewallApplicationFilter { [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject) process { [pscustomobject]@{Program=$script:rule.Program;Package='Any'} } }
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
. (Join-Path $PSScriptRoot '../src/admin/firewall_helper.ps1')
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
Write-Output "Firewall helper fake checks: $script:checks PASS; host firewall untouched"
