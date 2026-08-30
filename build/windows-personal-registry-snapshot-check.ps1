param(
  [Parameter(Mandatory = $true)][string]$SnapshotRoot,
  [Parameter(Mandatory = $true)][string]$InstallKey,
  [Parameter(Mandatory = $true)][string]$UninstallKey,
  [string]$UserRegistryHive = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Fail([string]$message) {
  throw [IO.InvalidDataException]::new($message)
}

function Assert-Protected([string]$path) {
  $item = Get-Item -Force -LiteralPath $path
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Fail "Reparse point rejected: $path" }
  $acl = Get-Acl -LiteralPath $path
  try { $owner = ([Security.Principal.NTAccount]$acl.Owner).Translate([Security.Principal.SecurityIdentifier]).Value }
  catch { $owner = $acl.Owner }
  $current = [Security.Principal.WindowsIdentity]::GetCurrent()
  $currentAdmin = ([Security.Principal.WindowsPrincipal]$current).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  if ($owner -notin @('S-1-5-18', 'S-1-5-32-544') -and -not ($currentAdmin -and $owner -eq $current.User.Value)) {
    Fail "Recovery owner is not protected: $path"
  }
  $required = @('S-1-5-18', 'S-1-5-32-544')
  $seen = @{}
  foreach ($rule in $acl.Access) {
    try { $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value }
    catch { Fail "Unresolved recovery trustee: $path" }
    if ($sid -in $required -and $rule.AccessControlType -eq 'Allow' -and
        (([int64]$rule.FileSystemRights -band [int64][Security.AccessControl.FileSystemRights]::FullControl) -eq
         [int64][Security.AccessControl.FileSystemRights]::FullControl)) { $seen[$sid] = $true }
    if ($sid -notin $required -and $rule.AccessControlType -eq 'Allow' -and
        (([int64]$rule.FileSystemRights -band 0x000D01FF) -ne 0)) { Fail "Non-administrator can modify recovery: $path" }
  }
  foreach ($sid in $required) { if (-not $seen[$sid]) { Fail "Required recovery ACL missing: $sid on $path" } }
}

function Read-State([string]$name, [string]$text) {
  $match = [regex]::Match($text, "(?m)^$([regex]::Escape($name))=([^\r\n]+)\r?$")
  if (-not $match.Success) { Fail "Missing recovery state: $name" }
  return $match.Groups[1].Value
}

function Read-State-OrZero([string]$name, [string]$text) {
  $match = [regex]::Match($text, "(?m)^$([regex]::Escape($name))=([01])\r?$")
  if (-not $match.Success) { return '0' }
  return $match.Groups[1].Value
}

function Assert-Export([string]$path, [string]$expectedHive, [string]$expectedKey, [bool]$required) {
  if (-not $required) {
    if (Test-Path -LiteralPath $path) { Fail "Unexpected registry export: $path" }
    return
  }
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { Fail "Missing registry export: $path" }
  Assert-Protected $path
  $lines = @(Get-Content -LiteralPath $path)
  if ($lines.Count -lt 2 -or $lines[0] -cne 'Windows Registry Editor Version 5.00') { Fail "Invalid registry export header: $path" }
  $sections = @($lines | Where-Object { $_ -cmatch '^\[.*\]$' })
  $expected = "[$expectedHive\$expectedKey]"
  if ($sections.Count -ne 1 -or $sections[0] -cne $expected) { Fail "Registry export does not contain exactly the expected fixed key: $path" }
}

Assert-Protected $SnapshotRoot
$snapshotItem = Get-Item -Force -LiteralPath $SnapshotRoot
if (-not $snapshotItem.PSIsContainer) { Fail 'Registry snapshot root is not a protected directory' }
$allowed = @(
  '.personal-registry-state.ini',
  '.personal-install.reg',
  '.personal-uninstall.reg',
  '.personal-install.snapshot-verify.reg',
  '.personal-uninstall.snapshot-verify.reg',
  '.personal-install.verify.reg',
  '.personal-uninstall.verify.reg',
  '.personal-hkcu-install.reg',
  '.personal-hkcu-uninstall.reg',
  '.personal-hkcu-install.snapshot-verify.reg',
  '.personal-hkcu-uninstall.snapshot-verify.reg',
  '.personal-hkcu-install.verify.reg',
  '.personal-hkcu-uninstall.verify.reg'
)
if (@(Get-ChildItem -Force -LiteralPath $SnapshotRoot | Where-Object { $_.Name -notin $allowed }).Count -ne 0) {
  Fail 'Unexpected entry in protected registry snapshot root'
}
$statePath = Join-Path $SnapshotRoot '.personal-registry-state.ini'
Assert-Protected $statePath
$state = Get-Content -Raw -LiteralPath $statePath
if ((Read-State 'complete' $state) -cne '1') { Fail 'Registry snapshot is incomplete' }
$installPresent = (Read-State 'installRegistry' $state) -ceq '1'
$uninstallPresent = (Read-State 'uninstallRegistry' $state) -ceq '1'
$hkcuInstallPresent = (Read-State-OrZero 'hkcuInstallRegistry' $state) -ceq '1'
$hkcuUninstallPresent = (Read-State-OrZero 'hkcuUninstallRegistry' $state) -ceq '1'
$serviceState = Read-State 'serviceState' $state
if (($hkcuInstallPresent -or $hkcuUninstallPresent) -and
    $UserRegistryHive -cnotmatch '^HKEY_USERS\\S-1-5-21-(?:[0-9]+-){3}[0-9]+$') {
  Fail 'Registry snapshot does not bind a valid interactive user hive'
}
if ($serviceState -notin @('absent', 'stopped', 'running', 'partial-stopped', 'partial-running')) { Fail 'Invalid predecessor service state in registry snapshot' }
Assert-Export (Join-Path $SnapshotRoot '.personal-install.reg') 'HKEY_LOCAL_MACHINE' $InstallKey $installPresent
Assert-Export (Join-Path $SnapshotRoot '.personal-uninstall.reg') 'HKEY_LOCAL_MACHINE' $UninstallKey $uninstallPresent
Assert-Export (Join-Path $SnapshotRoot '.personal-hkcu-install.reg') $UserRegistryHive $InstallKey $hkcuInstallPresent
Assert-Export (Join-Path $SnapshotRoot '.personal-hkcu-uninstall.reg') $UserRegistryHive $UninstallKey $hkcuUninstallPresent
Write-Output 'Talking Quill registry snapshot verified.'
