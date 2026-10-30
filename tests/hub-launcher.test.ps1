param([string]$Binary=(Join-Path $PSScriptRoot '../target/debug/moyai-hub.exe'))
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot '../scripts/component-install.ps1')
$fixture=Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) ('project_sandbox/hub-launcher-tests/'+[Guid]::NewGuid().ToString('N'))
function Get-MoyaiComponentUserReceiptPath([string]$Component) { Join-Path $fixture 'installations/hub.json' }
$Binary=[IO.Path]::GetFullPath($Binary)
$errorText=''
try { Start-MoyaiHub $Binary @('--unsupported-launcher-test-option') } catch { $errorText=$_.Exception.Message }
if ($errorText -notmatch 'unsupported-launcher-test-option' -or $errorText -notmatch 'stderr.log') { throw "Launch failure lost the original reason or its log: $errorText" }
$before=@(Get-ChildItem -LiteralPath (Join-Path $fixture 'installations/logs') -File)
if ($before.Count -ne 2) { throw 'Failure must retain stdout and stderr logs.' }
Start-MoyaiHub $Binary @('--help')
$after=@(Get-ChildItem -LiteralPath (Join-Path $fixture 'installations/logs') -File)
if ($after.Count -ne 2) { throw 'Successful launch must not accumulate logs or remove prior failure evidence.' }
if (Test-Path -LiteralPath (Join-Path $fixture 'hub')) { throw 'Logging must not populate the Hub profile.' }
Write-Output 'PASS: launch errors retain the real reason, successful invocation clears only its own logs.'
