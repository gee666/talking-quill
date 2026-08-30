param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('fresh-install', 'install', 'install-commit', 'install-retire-backup', 'install-rollback', 'uninstall')]
  [string]$Mode,
  [Parameter(Mandatory = $true)]
  [string]$FailurePoint,
  [string]$NativeProgramData = '',
  [string]$ExpectedGatewayHash = '',
  [string]$ExpectedOwnerHash = '',
  [string]$ExpectedLayoutDigest = '',
  [string]$TestRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class TalkingQuillAtomicFile {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern bool MoveFileEx(string existingPath, string newPath, int flags);
}
'@

$serviceName = 'TalkingQuillKeyboardAuthority'
$isolatedTest = $TestRoot -ne ''
function Get-NativeProgramFiles {
  $baseKey = $null
  $currentVersion = $null
  try {
    # Read the 64-bit registry view explicitly. The 32-bit NSIS process starts
    # 32-bit Windows PowerShell, where SpecialFolder.ProgramFiles and normal
    # registry access both redirect to Program Files (x86).
    $baseKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
      [Microsoft.Win32.RegistryHive]::LocalMachine,
      [Microsoft.Win32.RegistryView]::Registry64
    )
    $currentVersion = $baseKey.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion', $false)
    if ($null -eq $currentVersion) {
      throw 'Windows did not expose its native Program Files registry record.'
    }
    $path = [string]$currentVersion.GetValue('ProgramFilesDir', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ([String]::IsNullOrWhiteSpace($path) -or -not [IO.Path]::IsPathRooted($path)) {
      throw 'Windows returned an invalid native Program Files directory.'
    }
    return [IO.Path]::GetFullPath($path).TrimEnd('\')
  } finally {
    if ($null -ne $currentVersion) { $currentVersion.Dispose() }
    if ($null -ne $baseKey) { $baseKey.Dispose() }
  }
}
if ($isolatedTest) {
  if ($env:TALKING_QUILL_MACHINE_CLEANUP_TEST_ONLY -ne '1') {
    throw 'Isolated machine-cleanup roots are test-only.'
  }
  $machineRoot = [IO.Path]::GetFullPath($TestRoot).TrimEnd('\')
  if (-not (Test-Path -LiteralPath (Join-Path $machineRoot '.talking-quill-cleanup-test-root') -PathType Leaf)) {
    throw 'The isolated machine-cleanup root is not armed.'
  }
} else {
  $machineRoot = Get-NativeProgramFiles
}
$installRoot = [IO.Path]::GetFullPath((Join-Path $machineRoot 'Talking Quill')).TrimEnd('\')
$backupRoot = [IO.Path]::GetFullPath((Join-Path $machineRoot '.Talking Quill.stage1-backup')).TrimEnd('\')
$orphanRoot = [IO.Path]::GetFullPath((Join-Path $machineRoot '.Talking Quill.stage1-ambiguous-replacement')).TrimEnd('\')
$transactionPath = [IO.Path]::GetFullPath((Join-Path $machineRoot '.Talking Quill.stage1-transaction.json'))
$legacyAuthorityRoot = if ($isolatedTest) {
  [IO.Path]::GetFullPath((Join-Path $machineRoot 'retired-authority')).TrimEnd('\')
} else {
  if ([String]::IsNullOrWhiteSpace($NativeProgramData) -or -not [IO.Path]::IsPathRooted($NativeProgramData)) {
    throw 'The native ProgramData path was not supplied by the installer Known Folder lookup.'
  }
  [IO.Path]::GetFullPath((Join-Path $NativeProgramData 'Talking Quill\KeyboardAuthority')).TrimEnd('\')
}

function Assert-PlainOwnedTree {
  param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
  if (-not (Test-Path -LiteralPath $Path)) { return }
  $root = Get-Item -Force -LiteralPath $Path
  if (($root.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Label root is a reparse point and will not be modified."
  }
  $reparse = @(Get-ChildItem -Force -Recurse -LiteralPath $Path | Where-Object {
    ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
  })
  if ($reparse.Count -ne 0) { throw "$Label contains a reparse point and will not be modified." }
}

function Assert-PlainOwnedFile {
  param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
  if (-not (Test-Path -LiteralPath $Path)) { return }
  $item = Get-Item -Force -LiteralPath $Path
  if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Label is not a plain file and will not be modified."
  }
}

function Get-PackageLayoutDigest {
  param([Parameter(Mandatory = $true)]$Manifest)
  $stream = New-Object IO.MemoryStream
  $prefix = [Text.Encoding]::UTF8.GetBytes("talking-quill/package-layout/v1`0")
  $stream.Write($prefix, 0, $prefix.Length)
  function Add-LayoutField {
    param([string]$Name, [string]$Value)
    $nameBytes = [Text.Encoding]::UTF8.GetBytes($Name)
    $valueBytes = [Text.Encoding]::UTF8.GetBytes($Value)
    $header = [byte[]]::new(6)
    $header[0] = [byte](($nameBytes.Length -shr 8) -band 255)
    $header[1] = [byte]($nameBytes.Length -band 255)
    $header[2] = [byte](($valueBytes.Length -shr 24) -band 255)
    $header[3] = [byte](($valueBytes.Length -shr 16) -band 255)
    $header[4] = [byte](($valueBytes.Length -shr 8) -band 255)
    $header[5] = [byte]($valueBytes.Length -band 255)
    $stream.Write($header, 0, $header.Length)
    $stream.Write($nameBytes, 0, $nameBytes.Length)
    $stream.Write($valueBytes, 0, $valueBytes.Length)
  }
  Add-LayoutField version ([string]$Manifest.version)
  Add-LayoutField platform ([string]$Manifest.platform)
  Add-LayoutField architecture ([string]$Manifest.architecture)
  Add-LayoutField ownerMode ([string]$Manifest.ownerMode)
  foreach ($role in @($Manifest.roles)) {
    Add-LayoutField role "$($role.role)`0$($role.path)`0$($role.sha256)`0$([string]$role.suppressionCapable).ToLowerInvariant()"
  }
  $predecessor = if ($null -eq $Manifest.predecessor) { '' } else {
    $p = $Manifest.predecessor
    "{`"platform`":`"$($p.platform)`",`"architecture`":`"$($p.architecture)`",`"version`":`"$($p.version)`",`"releaseBuildDigest`":`"$($p.releaseBuildDigest)`",`"gatewaySha256`":`"$($p.gatewaySha256)`",`"ownerSha256`":`"$($p.ownerSha256)`"}"
  }
  Add-LayoutField predecessor $predecessor
  if ($Manifest.freshInstall -eq $true) { Add-LayoutField freshInstall 'true' }
  $sha = [Security.Cryptography.SHA256]::Create()
  try { return -join ($sha.ComputeHash($stream.ToArray()) | ForEach-Object { $_.ToString('x2') }) }
  finally { $sha.Dispose(); $stream.Dispose() }
}

function Assert-InstalledCandidatePayload {
  $manifestPath = Join-Path $installRoot 'resources\keyboard-owner-release-v1.json'
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw 'The extracted candidate release manifest is missing.'
  }
  $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
  $roles = @($manifest.roles)
  if ($manifest.schemaVersion -ne 1 -or $manifest.kind -cne 'talking-quill-local-owner-release' -or $manifest.platform -cne 'win' -or $roles.Count -ne 2) {
    throw 'The extracted candidate release manifest is invalid.'
  }
  $gateway = $roles[0]
  $owner = $roles[1]
  if ($gateway.role -cne 'gateway' -or $gateway.path -cne 'resources/helper/talking-quill-helper.exe' -or $gateway.suppressionCapable -ne $false -or $owner.role -cne 'owner' -or $owner.path -cne 'resources/helper/talking-quill-keyboard-owner.exe' -or $owner.suppressionCapable -ne $true) {
    throw 'The extracted candidate role layout is invalid.'
  }
  $layout = Get-PackageLayoutDigest $manifest
  if ($layout -cne $manifest.packageLayoutDigest -or $manifest.releaseBuildDigest -cne $layout) {
    throw 'The extracted candidate package layout digest is invalid.'
  }
  foreach ($role in @($gateway, $owner)) {
    $path = Join-Path $installRoot $role.path
    $item = Get-Item -Force -LiteralPath $path
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $role.sha256) {
      throw "The extracted $($role.role) bytes do not match the declared payload."
    }
  }
  if ($ExpectedLayoutDigest -ne '' -and ($ExpectedLayoutDigest -cne $layout -or $ExpectedGatewayHash -cne $gateway.sha256 -or $ExpectedOwnerHash -cne $owner.sha256)) {
    throw 'The extracted NSIS payload does not match the signed update authorization.'
  }
}

function Normalize-RuntimeExecutablePath {
  param([string]$Path)
  if ([String]::IsNullOrWhiteSpace($Path)) { return '' }
  if ($Path.StartsWith('\\?\', [StringComparison]::Ordinal)) {
    return $Path.Substring(4)
  }
  return $Path
}

function Get-ActiveRuntime {
  if ($isolatedTest) { return @() }
  return @(Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object {
    $path = Normalize-RuntimeExecutablePath -Path ([string]$_.ExecutablePath)
    $path -and ($path -ieq (Join-Path $installRoot 'Talking Quill.exe') -or
      $path -ieq (Join-Path $installRoot 'resources\helper\talking-quill-helper.exe') -or
      $path -ieq (Join-Path $installRoot 'resources\helper\talking-quill-keyboard-owner.exe'))
  })
}

function Request-PlannedRuntimeExit {
  if ($isolatedTest -or $Mode -notin @('install', 'uninstall')) { return }
  $active = @(Get-ActiveRuntime)
  if ($active.Count -eq 0) { return }
  $application = Join-Path $installRoot 'Talking Quill.exe'
  Assert-PlainOwnedFile -Path $application -Label 'Talking Quill application'
  if (-not (Test-Path -LiteralPath $application -PathType Leaf)) {
    throw 'Talking Quill is running but its application executable is unavailable. Close it before retrying.'
  }
  # Shell.Application delegates launch to the interactive Explorer shell instead
  # of starting the application with this installer's elevated token.
  $shell = New-Object -ComObject Shell.Application
  $shell.ShellExecute($application, '--talking-quill-request-machine-quit', $installRoot, 'open', 0)
  for ($attempt = 0; $attempt -lt 300; $attempt += 1) {
    if (@(Get-ActiveRuntime).Count -eq 0) { return }
    Start-Sleep -Milliseconds 100
  }
  throw 'Talking Quill did not confirm neutral planned exit. Release held keys, close the app, and retry.'
}

function Assert-NoActiveRuntime {
  if (@(Get-ActiveRuntime).Count -ne 0) {
    throw 'Talking Quill is still running. Close it and wait for the keyboard owner to drain before retrying.'
  }
}

function Remove-LegacyService {
  if ($isolatedTest) { return }
  $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
  if ($null -eq $service) { return }
  if ($service.Status -ne [ServiceProcess.ServiceControllerStatus]::Stopped) {
    Stop-Service -InputObject $service -ErrorAction Stop
    $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(30))
  }
  & "$env:SystemRoot\System32\sc.exe" delete $serviceName | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'The retired Talking Quill keyboard service could not be deleted.' }
  for ($attempt = 0; $attempt -lt 50; $attempt += 1) {
    if ($null -eq (Get-Service -Name $serviceName -ErrorAction SilentlyContinue)) { return }
    Start-Sleep -Milliseconds 100
  }
  throw 'The retired Talking Quill keyboard service is pending deletion. Restart Windows, then retry.'
}

function Read-InstallTransaction {
  if (-not (Test-Path -LiteralPath $transactionPath)) { return $null }
  Assert-PlainOwnedFile -Path $transactionPath -Label 'Talking Quill transaction marker'
  $record = Get-Content -Raw -LiteralPath $transactionPath | ConvertFrom-Json
  $keys = @($record.PSObject.Properties.Name | Sort-Object)
  if (($keys -join ',') -ne 'hadPredecessor,schemaVersion,state' -or
    $record.schemaVersion -ne 1 -or
    $record.state -notin @('staging', 'prepared', 'restoring', 'committed') -or
    $record.hadPredecessor -isnot [bool]) {
    throw 'The Talking Quill install transaction marker is invalid and was preserved.'
  }
  return $record
}

function Write-InstallTransaction {
  param([Parameter(Mandatory = $true)][string]$State, [Parameter(Mandatory = $true)][bool]$HadPredecessor)
  $temporary = "$transactionPath.$PID.tmp"
  try {
    $json = @{ schemaVersion = 1; state = $State; hadPredecessor = $HadPredecessor } |
      ConvertTo-Json -Compress
    [IO.File]::WriteAllText($temporary, $json, [Text.UTF8Encoding]::new($false))
    # MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH keeps the state change
    # atomic and asks Windows to flush the rename before returning.
    if (-not [TalkingQuillAtomicFile]::MoveFileEx($temporary, $transactionPath, 9)) {
      throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
    }
  } finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -Force -LiteralPath $temporary }
  }
}

function Remove-InstallTransaction {
  if (Test-Path -LiteralPath $transactionPath) {
    Remove-Item -Force -LiteralPath $transactionPath
  }
}

function Clear-OwnedTreeContents {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not (Test-Path -LiteralPath $Path)) { return }
  foreach ($child in @(Get-ChildItem -Force -LiteralPath $Path)) {
    Remove-Item -LiteralPath $child.FullName -Recurse -Force
  }
}

function Remove-EmptyRootBestEffort {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (-not (Test-Path -LiteralPath $Path)) { return }
  if (@(Get-ChildItem -Force -LiteralPath $Path).Count -ne 0) {
    throw "Talking Quill could not empty its owned machine directory: $Path"
  }
  try { Remove-Item -LiteralPath $Path -Force -ErrorAction Stop } catch {
    # NSIS or an antivirus scanner may retain a directory handle even though no
    # installed image remains. An empty fixed root is safe for extraction and
    # customRemoveFiles retries its final removal after the uninstaller exits.
  }
}

function Move-OwnedTreeContents {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination,
    [string]$InterruptAfterFirstChild = 'none',
    [bool]$AllowDestinationContents = $false
  )
  if (-not (Test-Path -LiteralPath $Source)) { return }
  if (Test-Path -LiteralPath $Destination) {
    if (-not $AllowDestinationContents -and @(Get-ChildItem -Force -LiteralPath $Destination).Count -ne 0) {
      throw "Talking Quill recovery destination is not empty: $Destination"
    }
  } else {
    New-Item -ItemType Directory -Path $Destination | Out-Null
  }
  $moved = 0
  foreach ($child in @(Get-ChildItem -Force -LiteralPath $Source | Sort-Object Name)) {
    $target = Join-Path $Destination $child.Name
    if (Test-Path -LiteralPath $target) {
      throw "Talking Quill recovery collision was preserved: $target"
    }
    Move-Item -LiteralPath $child.FullName -Destination $Destination
    $moved += 1
    if ($moved -eq 1 -and $InterruptAfterFirstChild -eq 'after-first-predecessor-child') {
      throw 'Injected install interruption after the first predecessor child.'
    }
    if ($moved -eq 1 -and $InterruptAfterFirstChild -eq 'after-first-restored-child') {
      throw 'Injected rollback interruption after the first restored predecessor child.'
    }
  }
  Remove-EmptyRootBestEffort -Path $Source
}

function Resolve-PendingInstallTransaction {
  param([Parameter(Mandatory = $true)][ValidateSet('recover', 'preserve-committed')][string]$CommittedAction)
  $transaction = Read-InstallTransaction
  if ($null -eq $transaction) {
    if (Test-Path -LiteralPath $backupRoot) {
      if (-not (Test-Path -LiteralPath $installRoot)) {
        Move-OwnedTreeContents -Source $backupRoot -Destination $installRoot
        return
      }
      # Older installers could leave an unmarked backup after either interrupted
      # replacement or failed retirement. Preserve both candidates, restore the
      # predecessor, and let a successful new commit retire the quarantine.
      if (Test-Path -LiteralPath $orphanRoot) {
        throw 'Talking Quill has multiple ambiguous replacement trees; all were preserved.'
      }
      Move-OwnedTreeContents -Source $installRoot -Destination $orphanRoot
      Move-OwnedTreeContents -Source $backupRoot -Destination $installRoot
    }
    return
  }
  if ($transaction.state -eq 'staging') {
    if (-not $transaction.hadPredecessor) {
      throw 'A fresh install cannot have a predecessor-staging transaction.'
    }
    $installExists = Test-Path -LiteralPath $installRoot
    $backupExists = Test-Path -LiteralPath $backupRoot
    if (-not $installExists -and -not $backupExists) {
      throw 'The staging Talking Quill transaction has no predecessor tree; recovery state was preserved.'
    }
    if (-not $installExists) {
      New-Item -ItemType Directory -Path $installRoot | Out-Null
    }
    if ($backupExists) {
      # A staging interruption occurs before electron-builder copies replacement
      # files. Restore the split predecessor as a union. Never clear either side
      # and never overwrite a collision.
      Move-OwnedTreeContents -Source $backupRoot -Destination $installRoot -AllowDestinationContents $true
    }
    Remove-InstallTransaction
    return
  }
  if ($transaction.state -eq 'restoring') {
    if (-not $transaction.hadPredecessor) {
      throw 'A fresh install cannot have a predecessor-restoring transaction.'
    }
    if (-not (Test-Path -LiteralPath $backupRoot)) {
      throw 'The restoring Talking Quill transaction has no recovery tree; recovery state was preserved.'
    }
    if (-not (Test-Path -LiteralPath $installRoot)) {
      New-Item -ItemType Directory -Path $installRoot | Out-Null
    }
    # Resume by joining the already-restored children with those still in backup.
    # A collision stops recovery without overwriting either candidate.
    Move-OwnedTreeContents -Source $backupRoot -Destination $installRoot -AllowDestinationContents $true -InterruptAfterFirstChild $FailurePoint
    Remove-InstallTransaction
    return
  }
  if ($transaction.state -eq 'committed') {
    if (-not (Test-Path -LiteralPath $installRoot)) {
      throw 'The committed Talking Quill replacement is missing; recovery state was preserved.'
    }
    if ($CommittedAction -eq 'preserve-committed') { return }
    if (Test-Path -LiteralPath $backupRoot) {
      Remove-Item -LiteralPath $backupRoot -Recurse -Force
    }
    if (Test-Path -LiteralPath $orphanRoot) {
      Remove-Item -LiteralPath $orphanRoot -Recurse -Force
    }
    Remove-InstallTransaction
    return
  }

  if ($transaction.hadPredecessor) {
    if (-not (Test-Path -LiteralPath $backupRoot)) {
      throw 'The prepared Talking Quill transaction has no complete predecessor recovery tree; recovery state was preserved.'
    }
    if (Test-Path -LiteralPath $installRoot) {
      Clear-OwnedTreeContents -Path $installRoot
    }
    Write-InstallTransaction -State restoring -HadPredecessor $true
    if (-not (Test-Path -LiteralPath $installRoot)) {
      New-Item -ItemType Directory -Path $installRoot | Out-Null
    }
    Move-OwnedTreeContents -Source $backupRoot -Destination $installRoot -AllowDestinationContents $true -InterruptAfterFirstChild $FailurePoint
  } else {
    if (Test-Path -LiteralPath $backupRoot) {
      throw 'A fresh-install transaction unexpectedly has a recovery tree; both were preserved.'
    }
    if (Test-Path -LiteralPath $installRoot) {
      Clear-OwnedTreeContents -Path $installRoot
      Remove-EmptyRootBestEffort -Path $installRoot
    }
  }
  Remove-InstallTransaction
}

Request-PlannedRuntimeExit
Assert-NoActiveRuntime
Assert-PlainOwnedTree -Path $installRoot -Label 'Talking Quill install'
Assert-PlainOwnedTree -Path $backupRoot -Label 'Talking Quill recovery'
Assert-PlainOwnedTree -Path $orphanRoot -Label 'Talking Quill ambiguous replacement'
Assert-PlainOwnedFile -Path $transactionPath -Label 'Talking Quill transaction marker'
Assert-PlainOwnedTree -Path $legacyAuthorityRoot -Label 'Retired Talking Quill authority data'

switch ($Mode) {
  'fresh-install' {
    # A fresh artifact has no predecessor authorization. It must never become
    # an update or repair merely because protected machine state already exists.
    if ((Test-Path -LiteralPath $installRoot) -or
        (Test-Path -LiteralPath $backupRoot) -or
        (Test-Path -LiteralPath $orphanRoot) -or
        (Test-Path -LiteralPath $transactionPath)) {
      throw 'A protected Talking Quill installation already exists. Use a predecessor-authorized update or uninstall it first.'
    }
    Write-InstallTransaction -State prepared -HadPredecessor $false
  }
  'install' {
    Resolve-PendingInstallTransaction -CommittedAction recover
    if ($FailurePoint -eq 'before-program-files-replace') {
      throw "Injected install failure: $FailurePoint"
    }
    $hadPredecessor = Test-Path -LiteralPath $installRoot
    if ($hadPredecessor) {
      Write-InstallTransaction -State staging -HadPredecessor $true
      Move-OwnedTreeContents -Source $installRoot -Destination $backupRoot -InterruptAfterFirstChild $FailurePoint
      Write-InstallTransaction -State prepared -HadPredecessor $true
    } else {
      Write-InstallTransaction -State prepared -HadPredecessor $false
    }
  }
  'install-rollback' {
    Resolve-PendingInstallTransaction -CommittedAction preserve-committed
  }
  'install-commit' {
    if ($FailurePoint -eq 'after-program-files-copy') {
      throw "Injected commit failure: $FailurePoint"
    }
    if (-not (Test-Path -LiteralPath $installRoot)) {
      throw 'The Talking Quill replacement is missing at the commit boundary.'
    }
    # Validate the exact bytes extracted from the NSIS app archive before the
    # transaction can become committed or the predecessor backup can retire.
    # Isolated transaction tests use marker files instead of a packaged tree.
    if (-not $isolatedTest) { Assert-InstalledCandidatePayload }
    $transaction = Read-InstallTransaction
    if ($null -eq $transaction -or $transaction.state -ne 'prepared') {
      throw 'Talking Quill commit requires a durable prepared transaction.'
    }
    Write-InstallTransaction -State committed -HadPredecessor $transaction.hadPredecessor
  }
  'install-retire-backup' {
    $transaction = Read-InstallTransaction
    if ($null -eq $transaction -or $transaction.state -ne 'committed') {
      throw 'Talking Quill cannot retire machine state without a durable committed replacement.'
    }
    # The replacement is durably committed before retired authority cleanup.
    # Failure here preserves the committed replacement and recovery tree for a
    # later repair instead of damaging the predecessor before rollback is safe.
    Remove-LegacyService
    if (Test-Path -LiteralPath $legacyAuthorityRoot) {
      Remove-Item -Force -Recurse -LiteralPath $legacyAuthorityRoot
    }
    Resolve-PendingInstallTransaction -CommittedAction recover
  }
  'uninstall' {
    Resolve-PendingInstallTransaction -CommittedAction recover
    Remove-LegacyService
    if (Test-Path -LiteralPath $legacyAuthorityRoot) {
      Remove-Item -Force -Recurse -LiteralPath $legacyAuthorityRoot
    }
    if ((Test-Path -LiteralPath $backupRoot) -or (Test-Path -LiteralPath $transactionPath)) {
      throw 'Talking Quill recovery state remains and uninstall was stopped before deleting the install root.'
    }
    if (Test-Path -LiteralPath $installRoot) {
      Clear-OwnedTreeContents -Path $installRoot
      Remove-EmptyRootBestEffort -Path $installRoot
    }
    if (Test-Path -LiteralPath $orphanRoot) {
      Remove-Item -LiteralPath $orphanRoot -Recurse -Force
    }
  }
}

Write-Output "Talking Quill machine cleanup completed: $Mode"
