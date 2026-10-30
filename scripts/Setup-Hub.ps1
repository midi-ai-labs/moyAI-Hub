param(
  [switch]$Uninstall,
  [switch]$CheckOnly,
  [switch]$NoShortcuts,
  [switch]$SkipStartupCheck,
  [switch]$ResetHubData,
  [switch]$Interactive,
  [switch]$LaunchOnly,
  [Nullable[int]]$WebPort,
  [string]$HubDataDir = ''
)
$ErrorActionPreference = 'Stop'
try {
  if ($null -ne $WebPort -and ($WebPort -lt 1 -or $WebPort -gt 65535)) { throw 'Web管理ポートには1～65535の固定ポートを指定してください。' }
  $packageRoot = Split-Path -Parent $PSScriptRoot
  if (-not (Test-Path -LiteralPath (Join-Path $packageRoot 'deployment.json') -PathType Leaf)) {
    if ($LaunchOnly) { throw 'この展開先から起動専用の操作はできません。導入済みならスタートメニューの「moyAI Team Management」、未導入ならSetup-Hub.cmdを使ってください。' }
    $parentSetup = Join-Path (Split-Path -Parent $packageRoot) 'scripts/Setup-moyAI.ps1'
    if (Test-Path -LiteralPath $parentSetup -PathType Leaf) {
      $forward = @{ Components=@('Hub'); Uninstall=$Uninstall; CheckOnly=$CheckOnly; NoShortcuts=$NoShortcuts; SkipStartupCheck=$SkipStartupCheck; ResetHubData=$ResetHubData; Interactive=$Interactive; HubDataDir=$HubDataDir }
      if ($null -ne $WebPort) { $forward.WebPort = $WebPort }
      & $parentSetup @forward
      exit $LASTEXITCODE
    }
    throw 'Hubの配布情報がありません。ZIP全体を展開し、Setup-Hub.cmdを開いてください。'
  }
  . (Join-Path $PSScriptRoot 'component-install.ps1')
  if ($LaunchOnly) {
    if ($Uninstall -or $CheckOnly -or $ResetHubData) { throw '起動と削除・確認は同時に指定できません。' }
    $installed = Read-MoyaiInstalledComponent 'Hub'
    if ($null -eq $installed) { throw 'Hubが導入されていません。Setup-Hub.cmdを実行してください。' }
    $executable = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'moyAI/Hub/moyai-hub.exe'
    $arguments = @('--launch')
    if ($null -ne $WebPort) { $arguments += @('--web-port', [string]$WebPort) }
    if ($HubDataDir) { if ($HubDataDir.Contains('"')) { throw 'Hub保存先に引用符は指定できません。' }; $arguments += @('--data-dir', ('"' + [IO.Path]::GetFullPath($HubDataDir) + '"')) }
    Start-MoyaiHub $executable $arguments
  } else {
    $invoke = @{ SourceRoot=$packageRoot; Components=@('Hub'); Uninstall=$Uninstall; CheckOnly=$CheckOnly; NoShortcuts=$NoShortcuts; SkipStartupCheck=$SkipStartupCheck; ResetHubData=$ResetHubData; Interactive=$Interactive; HubDataDir=$HubDataDir }
    if ($null -ne $WebPort) { $invoke.WebPort = $WebPort }
    Invoke-MoyaiComponentUserSetup @invoke
  }
  exit 0
} catch {
  $message='Hubの操作を完了できませんでした。' + [Environment]::NewLine + $_.Exception.Message
  Write-Host $message
  if ($LaunchOnly) {
    Add-Type -AssemblyName System.Windows.Forms
    [void][Windows.Forms.MessageBox]::Show($message, 'moyAI Hub', 'OK', 'Error')
  }
  exit 1
}
