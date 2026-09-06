param(
  [Parameter(Mandatory)][ValidateSet('x64','arm64')][string]$Architecture,
  [Parameter(Mandatory)][string]$Installer,
  [Parameter(Mandatory)][string]$Provenance,
  [Parameter(Mandatory)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# This is a destructive install/uninstall test, never a local-machine diagnostic.
if (-not $IsWindows -or $env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or $env:RUNNER_OS -cne 'Windows' -or $env:GITHUB_RUN_ID -cnotmatch '^[1-9][0-9]*$') { throw 'Requires a disposable GitHub-hosted Windows runner.' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Hosted lifecycle requires an elevated runner, not an interactive UAC test.' }
$native = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$nodeArch = & node -p 'process.arch'
if ($LASTEXITCODE -ne 0 -or $native -cne $Architecture -or $nodeArch -cne $Architecture) { throw 'Hosted lifecycle requires native Windows and Node architecture.' }
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$Provenance = (Resolve-Path -LiteralPath $Provenance).Path
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$projectTmp = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../tmp')) + [IO.Path]::DirectorySeparatorChar
if (-not $OutputDirectory.StartsWith($projectTmp, [StringComparison]::OrdinalIgnoreCase)) { throw 'Logs must remain under project tmp.' }
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
Start-Transcript -Path (Join-Path $OutputDirectory 'transcript.txt') | Out-Null
$pf = [Environment]::GetFolderPath('ProgramFiles')
$pd = [Environment]::GetFolderPath('CommonApplicationData')
$root = Join-Path $pf 'Talking Quill'
$registration = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill'
$maintenance = $null
$maintenanceHash = $null
$uninstalled = $false

function Get-MachineResidue {
  $paths = @($root, (Join-Path $pf 'Talking Quill Maintenance.exe'), (Join-Path $pd 'Talking Quill Update Recovery'), (Join-Path $pd 'Talking Quill\KeyboardAuthority'), (Join-Path $pd 'Talking Quill\.KeyboardAuthority.retirement-quarantine'))
  $paths += @(Get-ChildItem -LiteralPath $pf -Force | Where-Object { $_.Name -like '.Talking Quill.*' -or $_.Name -like 'Talking Quill Maintenance-*.exe' } | ForEach-Object FullName)
  $paths += @(Get-ChildItem -LiteralPath $pd -Force | Where-Object { $_.Name -like '.Talking Quill.*' } | ForEach-Object FullName)
  @($paths | Where-Object { Test-Path -LiteralPath $_ })
  foreach ($key in @($registration, 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill', 'HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe', 'HKLM:\Software\Talking Quill\RecoveryStateLockV1')) {
    if (Test-Path -LiteralPath $key) { $key }
  }
  if (Get-Service -Name 'TalkingQuillKeyboardAuthority' -ErrorAction SilentlyContinue) { 'legacy-service' }
  if (Get-ScheduledTask -TaskName 'TalkingQuillKeyboardAuthority' -ErrorAction SilentlyContinue) { 'legacy-task' }
  @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('Talking Quill.exe','talking-quill-helper.exe','talking-quill-keyboard-owner.exe') } | ForEach-Object { "process:$($_.ProcessId):$($_.Name)" })
}
function Run-Quiet([string]$Executable, [string]$Label) {
  $exitCode = [TqHostedMediumLauncher]::Launch($Executable, (Join-Path $OutputDirectory "$Label.launch.txt"), 180000)
  if ($exitCode -ne 0) { throw "$Label failed with exit code $exitCode." }
  return $exitCode
}
function Remove-OwnedInstall {
  if ($null -eq $maintenance -or $null -eq $maintenanceHash) { throw 'No authenticated maintenance registration was captured; refusing cleanup.' }
  $current = Get-ItemPropertyValue -LiteralPath $registration -Name QuietUninstallString
  if ($current -cne ('"' + $maintenance + '" /S') -or (Get-FileHash -LiteralPath $maintenance -Algorithm SHA256).Hash -cne $maintenanceHash) { throw 'Maintenance identity changed; refusing cleanup.' }
  Run-Quiet $maintenance 'uninstall'
}
try {
  # Record policy, but never change it. Hosted images may disable UAC and have no split token.
  Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' |
    Select-Object EnableLUA, ConsentPromptBehaviorAdmin, PromptOnSecureDesktop |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'uac-policy.json')
  Add-Type -Path (Join-Path $PSScriptRoot 'windows-hosted-medium-launcher.cs')
  $before = @(Get-MachineResidue)
  if ($before.Count -ne 0) { throw "Refusing to touch pre-existing machine state: $($before -join ', ')" }
  foreach ($path in @((Join-Path $env:APPDATA 'Talking Quill'), (Join-Path $env:LOCALAPPDATA 'Talking Quill'))) {
    if (Test-Path -LiteralPath $path) { throw 'Refusing to touch a runner with an existing user installation/profile.' }
  }
  $bindingPath = Join-Path $OutputDirectory 'binding.json'
  & node scripts/windows-hosted-lifecycle-evidence.mjs --prepare $Installer $Provenance $Architecture $bindingPath
  if ($LASTEXITCODE -ne 0) { throw 'Fresh production installer provenance verification failed.' }
  $binding = Get-Content -Raw -LiteralPath $bindingPath | ConvertFrom-Json
  $installExit = Run-Quiet $Installer 'install'
  $quiet = Get-ItemPropertyValue -LiteralPath $registration -Name QuietUninstallString
  if ($quiet -cnotmatch '^"(.+\\Talking Quill Maintenance-[0-9a-f]{32}\.exe)" /S$') { throw 'Quiet maintenance registration is not exact.' }
  $candidate = [IO.Path]::GetFullPath($Matches[1])
  if ([IO.Path]::GetDirectoryName($candidate) -cne $pf -or (Get-Item -LiteralPath $candidate).Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)) { throw 'Maintenance does not reside in the fixed native Program Files directory.' }
  $maintenanceHash = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash
  $maintenance = $candidate
  foreach ($file in $binding.files) {
    $installedPath = Join-Path $root $file.path
    $item = Get-Item -LiteralPath $installedPath
    if ($item.Attributes.HasFlag([IO.FileAttributes]::ReparsePoint) -or $item.Length -ne $file.size -or (Get-FileHash -LiteralPath $installedPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne $file.sha256) { throw "Installed file differs from authenticated native package: $($file.path)" }
  }
  $originalTemp = $env:TEMP
  $originalTmp = $env:TMP
  try {
    $env:TEMP = Join-Path $OutputDirectory 'runtime'
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Force $env:TEMP | Out-Null
    & node scripts/windows-package-lifecycle.mjs --arch $Architecture --mode installed --root $root 1> (Join-Path $OutputDirectory 'lifecycle.json') 2> (Join-Path $OutputDirectory 'lifecycle.stderr.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Installed application/helper/owner lifecycle failed.' }
  } finally {
    $env:TEMP = $originalTemp
    $env:TMP = $originalTmp
  }
  $lifecycle = Get-Content -Raw -LiteralPath (Join-Path $OutputDirectory 'lifecycle.json') | ConvertFrom-Json
  $uninstallExit = Remove-OwnedInstall
  $uninstalled = $true
  $deadline = [DateTime]::UtcNow.AddSeconds(60)
  do {
    $residue = @(Get-MachineResidue)
    if ($residue.Count -eq 0) { break }
    Start-Sleep -Seconds 1
  } while ([DateTime]::UtcNow -lt $deadline)
  if ($residue.Count -ne 0) { throw "Machine removal incomplete: $($residue -join ', ')" }
  $evidence = [ordered]@{ schemaVersion=1; kind='github-hosted-elevated-install-lifecycle'; result='passed'; host='github-hosted'; elevated=$true; uac='not-exercised'; workflowRunId=$env:GITHUB_RUN_ID; architecture=$Architecture; sourceCommit=$binding.sourceCommit; sourceTree=$binding.sourceTree; sourceTreeSha256=$binding.sourceTreeSha256; version=$binding.version; installer=$binding.installer; installerSha256=$binding.installerSha256; bytes=$binding.bytes; provenanceDocumentSha256=$binding.provenanceDocumentSha256; machineStateBefore='absent'; machineStateAfter='absent'; installExitCode=$installExit; uninstallExitCode=$uninstallExit; installedFilesVerified=$true; installedFileCount=$binding.files.Count; lifecycle=$lifecycle }
  $evidencePath = Join-Path $OutputDirectory "windows-hosted-lifecycle-$Architecture.json"
  $evidence | ConvertTo-Json -Depth 30 | Set-Content -Encoding utf8 $evidencePath
  & node scripts/windows-hosted-lifecycle-evidence.mjs --verify $Installer $Provenance $Architecture $evidencePath
  if ($LASTEXITCODE -ne 0) { throw 'Hosted lifecycle evidence verification failed.' }
} catch {
  $_ | Out-String | Set-Content -LiteralPath (Join-Path $OutputDirectory 'failure.txt')
  throw
} finally {
  if (-not $uninstalled -and $null -ne $maintenance) {
    try { Remove-OwnedInstall | Out-Null } catch { $_ | Out-String | Set-Content -LiteralPath (Join-Path $OutputDirectory 'cleanup-failure.txt') }
  }
  Stop-Transcript | Out-Null
}
