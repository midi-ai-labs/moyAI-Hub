# Embedded by the Hub. No caller-supplied script, file or cmdlet name is accepted.
$ErrorActionPreference = 'Stop'
function Test-MoyaiPrivate([string]$Address) {
    $ip = $null
    if (-not [Net.IPAddress]::TryParse($Address, [ref]$ip) -or $ip.AddressFamily -ne 'InterNetwork' -or $ip.ToString() -cne $Address) { return $false }
    $b = $ip.GetAddressBytes()
    return ($b[0] -eq 10 -or ($b[0] -eq 172 -and $b[1] -ge 16 -and $b[1] -le 31) -or ($b[0] -eq 192 -and $b[1] -eq 168))
}
function Assert-MoyaiSpec($s) {
    if ($s.name -cnotmatch '^moyai-hub-inbound-v1-[0-9a-f]{24}$' -or $s.program -notmatch '^[A-Za-z]:\\.+\.exe$' -or $s.program -match '["\r\n]' -or -not (Test-MoyaiPrivate $s.local_address)) { throw 'Invalid target' }
    if (@($s.local_ports).Count -ne 2 -or @($s.local_ports | Sort-Object -Unique).Count -ne 2 -or @($s.local_ports | Where-Object { [string]$_ -notmatch '^[0-9]{1,5}$' -or [int]$_ -lt 1 -or [int]$_ -gt 65535 }).Count -gt 0 -or $s.profile_mask -notin @(1,2,3)) { throw 'Invalid ports or profiles' }
    if (@($s.remote_addresses).Count -lt 1 -or @($s.remote_addresses).Count -gt 16) { throw 'Invalid scope' }
    foreach ($range in $s.remote_addresses) {
        if ($range -cnotmatch '^([0-9.]+)/([0-9]{1,2})$') { throw 'Invalid subnet' }
        $address=$Matches[1]; $prefix=[int]$Matches[2]
        if (-not (Test-MoyaiPrivate $address) -or $prefix -lt 8 -or $prefix -gt 32) { throw 'Invalid subnet' }
        $bytes=([Net.IPAddress]::Parse($address)).GetAddressBytes()
        $number=([uint64]$bytes[0]*16777216)+([uint64]$bytes[1]*65536)+([uint64]$bytes[2]*256)+$bytes[3]
        $size=[uint64][Math]::Pow(2,32-$prefix)
        if (($number % $size) -ne 0) { throw 'Subnet must be canonical' }
        $last=$number+$size-1
        $lastAddress='{0}.{1}.{2}.{3}' -f (($last -shr 24) -band 255),(($last -shr 16) -band 255),(($last -shr 8) -band 255),($last -band 255)
        if (-not (Test-MoyaiPrivate $lastAddress)) { throw 'Subnet exceeds private network' }
    }
}
function Get-MoyaiDescription($s) {
    $canonical=@($s.name,$s.program.ToLowerInvariant(),$s.local_address,(@($s.local_ports | Sort-Object { [int]$_ }) -join ','),(@($s.remote_addresses | Sort-Object) -join ','),[string]$s.profile_mask) -join "`n"
    $sha=[Security.Cryptography.SHA256]::Create()
    try { $hash=([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($canonical)))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
    return 'moyAI Hub inbound v1 '+$hash
}
function Get-MoyaiRule([string]$Name) {
    if ($Name -cnotmatch '^moyai-hub-inbound-v1-[0-9a-f]{24}$') { throw 'Invalid rule name' }
    $lookupErrors=@()
    $rows=@(Get-NetFirewallRule -PolicyStore PersistentStore -Name $Name -ErrorAction SilentlyContinue -ErrorVariable lookupErrors)
    foreach ($failure in $lookupErrors) { if ($failure.CategoryInfo.Category -ne 'ObjectNotFound') { throw $failure } }
    if ($rows.Count -eq 0) { return $null }
    if ($rows.Count -ne 1) { throw 'Ambiguous rule' }
    $r=$rows[0]; $app=$r | Get-NetFirewallApplicationFilter; $addr=$r | Get-NetFirewallAddressFilter; $port=$r | Get-NetFirewallPortFilter
    $service=$r | Get-NetFirewallServiceFilter; $iface=$r | Get-NetFirewallInterfaceFilter; $ifaceType=$r | Get-NetFirewallInterfaceTypeFilter; $security=$r | Get-NetFirewallSecurityFilter
    $spec=[ordered]@{name=[string]$r.Name;program=[string]$app.Program;local_address=(@($addr.LocalAddress) -join ',');local_ports=@($port.LocalPort | ForEach-Object { [int]$_ } | Sort-Object);remote_addresses=@($addr.RemoteAddress | ForEach-Object { if ($_ -match '^[0-9.]+$') { "$_/32" } else { [string]$_ } } | Sort-Object);profile_mask=[int]$r.Profile}
    $owned=$false
    try {
        Assert-MoyaiSpec $spec
        $owned=($r.DisplayName -ceq ('moyAI Hub TCP '+$Name.Substring(21)) -and $r.Group -ceq 'moyAI Hub managed inbound v1' -and $r.Description -ceq (Get-MoyaiDescription $spec) -and [string]$r.Enabled -eq 'True' -and [string]$r.Direction -eq 'Inbound' -and [string]$r.Action -eq 'Allow' -and [string]$r.EdgeTraversalPolicy -eq 'Block' -and [string]$r.LooseSourceMapping -eq 'False' -and [string]$r.LocalOnlyMapping -eq 'False' -and [string]$port.Protocol -in @('TCP','6') -and (@($port.RemotePort) -join ',') -eq 'Any' -and $service.Service -eq 'Any' -and (@($iface.InterfaceAlias) -join ',') -eq 'Any' -and $ifaceType.InterfaceType -eq 'Any' -and $app.Package -eq 'Any' -and $security.Authentication -eq 'NotRequired' -and $security.Encryption -eq 'NotRequired' -and [string]$security.OverrideBlockRules -eq 'False' -and $security.LocalUser -eq 'Any' -and $security.RemoteUser -eq 'Any' -and $security.RemoteMachine -eq 'Any')
    } catch { $owned=$false }
    return [ordered]@{spec=$spec;owned=[bool]$owned}
}
function Test-MoyaiSame($a,$b) {
    if ($null -eq $a -or $null -eq $b) { return ($null -eq $a -and $null -eq $b) }
    return ($a.name -ceq $b.name -and $a.program -ieq $b.program -and (Get-MoyaiDescription $a) -ceq (Get-MoyaiDescription $b))
}
function Invoke-MoyaiFirewall($request) {
    Assert-MoyaiSpec $request.spec
    $now=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    if ($request.action -notin @('apply','remove') -or $request.binary_sha256 -cnotmatch '^[0-9a-f]{64}$' -or $now -ge $request.expires_at_ms -or $request.expires_at_ms -gt $now+60000) { throw 'Expired or invalid request' }
    if ((Get-FileHash -LiteralPath $request.spec.program -Algorithm SHA256).Hash.ToLowerInvariant() -cne $request.binary_sha256) { throw 'Executable changed' }
    if ($null -ne $request.previous) { Assert-MoyaiSpec $request.previous; if ($request.previous.name -cne $request.spec.name -or $request.previous.program -ine $request.spec.program) { throw 'Owner changed' } }
    $actual=Get-MoyaiRule $request.spec.name
    if (($null -ne $actual -and -not $actual.owned) -or -not (Test-MoyaiSame $actual.spec $request.previous)) { throw 'Rule changed' }
    if ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -ge $request.expires_at_ms) { throw 'Expired request' }
    if ($request.action -eq 'remove') {
        if ($null -ne $actual) { Remove-NetFirewallRule -PolicyStore PersistentStore -Name $request.spec.name -Confirm:$false }
    } elseif (-not (Test-MoyaiSame $actual.spec $request.spec)) {
        $s=$request.spec
        $parameters=@{PolicyStore='PersistentStore';Name=$s.name;Description=(Get-MoyaiDescription $s);Enabled='True';Direction='Inbound';Action='Allow';EdgeTraversalPolicy='Block';Profile=[int]$s.profile_mask;Program=$s.program;Protocol='TCP';LocalAddress=$s.local_address;LocalPort=@($s.local_ports | ForEach-Object { [string]$_ });RemoteAddress=@($s.remote_addresses);RemotePort='Any';InterfaceType='Any';Authentication='NotRequired';Encryption='NotRequired';OverrideBlockRules=$false;LooseSourceMapping=$false;LocalOnlyMapping=$false}
        if ($null -eq $actual) { $parameters.DisplayName='moyAI Hub TCP '+$s.name.Substring(21); $parameters.Group='moyAI Hub managed inbound v1'; New-NetFirewallRule @parameters | Out-Null }
        else { Set-NetFirewallRule @parameters | Out-Null }
    }
    $after=Get-MoyaiRule $request.spec.name
    if ($request.action -eq 'remove') { if ($null -ne $after) { throw 'Removal not verified' } }
    elseif ($null -eq $after -or -not $after.owned -or -not (Test-MoyaiSame $after.spec $request.spec)) { throw 'Apply not verified' }
}
