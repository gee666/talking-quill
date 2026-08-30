param(
  [Parameter(Mandatory = $true)][string]$ProgramFilesRoot,
  [Parameter(Mandatory = $true)][string]$ProgramFilesRecovery,
  [Parameter(Mandatory = $true)][string]$ReleaseManifestPath,
  [ValidateRange(1, 200)][int]$RetryCount = 100,
  [ValidateRange(10, 1000)][int]$RetryDelayMilliseconds = 100
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class TalkingQuillProcessHandle {
  public const uint MinimalAccess = 0x00101001; // query-limited | terminate | synchronize
  public const uint WaitObject0 = 0;
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern IntPtr OpenProcess(uint access, bool inheritHandle, uint processId);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder path, ref uint size);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern bool TerminateProcess(IntPtr process, uint exitCode);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
  [DllImport("kernel32.dll")]
  public static extern bool CloseHandle(IntPtr handle);
}
'@

function Canonical([string]$Path) {
  return [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
}

$machineProgramFiles = if ([String]::IsNullOrWhiteSpace($env:ProgramW6432)) {
  $env:ProgramFiles
} else {
  $env:ProgramW6432
}
$fixedRoot = Canonical (Join-Path $machineProgramFiles 'Talking Quill')
$fixedRecovery = Canonical (Join-Path $machineProgramFiles '.Talking Quill.personal-recovery')
$root = Canonical $ProgramFilesRoot
$recovery = Canonical $ProgramFilesRecovery
if (-not [String]::Equals($root, $fixedRoot, [StringComparison]::OrdinalIgnoreCase) -or
    -not [String]::Equals($recovery, $fixedRecovery, [StringComparison]::OrdinalIgnoreCase)) {
  [Console]::Error.Write("invalid-fixed-path;root=$root;expectedRoot=$fixedRoot;recovery=$recovery;expectedRecovery=$fixedRecovery")
  exit 78
}
if (-not (Test-Path -LiteralPath $root -PathType Container)) {
  [Console]::Write('already-absent')
  exit 0
}
if (Test-Path -LiteralPath $recovery) {
  [Console]::Error.Write('recovery-already-exists')
  exit 75
}

$fixedManifest = Canonical (Join-Path $root 'resources\keyboard-owner-release-v1.json')
$manifestPath = Canonical $ReleaseManifestPath
if (-not [String]::Equals($manifestPath, $fixedManifest, [StringComparison]::OrdinalIgnoreCase)) {
  [Console]::Error.Write('invalid-release-manifest-path')
  exit 78
}
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$electronRole = @($manifest.roles | Where-Object { $_.role -ceq 'electron' })
if ($electronRole.Count -ne 1 -or $electronRole[0].path -cne 'Talking Quill.exe' -or
    $electronRole[0].sha256 -cnotmatch '^[0-9a-f]{64}$') {
  [Console]::Error.Write('invalid-electron-role')
  exit 78
}
$electronPath = Canonical (Join-Path $root $electronRole[0].path)
$actualHash = (Get-FileHash -LiteralPath $electronPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -cne $electronRole[0].sha256) {
  [Console]::Error.Write('electron-role-hash-mismatch')
  exit 78
}

# The root classifier authenticated the complete protected tree and manifest.
# Electron can create another renderer while an earlier process snapshot drains, so repeat exact
# image authentication before every bounded rename attempt. Never tree-kill descendants.
$prefix = $root + '\'
$drained = 0
$lastError = $null
for ($attempt = 1; $attempt -le $RetryCount; $attempt += 1) {
  $processes = @(Get-CimInstance Win32_Process | Where-Object {
    $null -ne $_.ExecutablePath -and
    [String]::Equals((Canonical $_.ExecutablePath), $electronPath, [StringComparison]::OrdinalIgnoreCase)
  })
  foreach ($candidate in $processes) {
    $nativeHandle = [IntPtr]::Zero
    try {
      # Open one minimally privileged process handle, query its kernel image, terminate that same
      # handle, and wait on that same handle. PID reuse cannot redirect a later operation.
      $nativeHandle = [TalkingQuillProcessHandle]::OpenProcess(
        [TalkingQuillProcessHandle]::MinimalAccess,
        $false,
        $candidate.ProcessId
      )
      if ($nativeHandle -eq [IntPtr]::Zero) {
        $live = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
        if ($null -eq $live) { $drained += 1; continue }
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
      }
      $image = [Text.StringBuilder]::new(32768)
      [uint32]$imageLength = $image.Capacity
      if (-not [TalkingQuillProcessHandle]::QueryFullProcessImageName($nativeHandle, 0, $image, [ref]$imageLength)) {
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
      }
      $handleImage = Canonical $image.ToString()
      if (-not [String]::Equals($handleImage, $electronPath, [StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      if (-not [TalkingQuillProcessHandle]::TerminateProcess($nativeHandle, 0)) {
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
      }
      if ([TalkingQuillProcessHandle]::WaitForSingleObject($nativeHandle, 5000) -ne [TalkingQuillProcessHandle]::WaitObject0) {
        throw "authenticated process did not exit: pid=$($candidate.ProcessId),path=$handleImage"
      }
      $drained += 1
    } catch [ArgumentException] {
      # The authenticated candidate exited before its handle was opened.
      $drained += 1
    } catch [InvalidOperationException] {
      $live = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
      if ($null -ne $live) { throw }
      $drained += 1
    } catch [System.ComponentModel.Win32Exception] {
      $live = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
      if ($null -ne $live) { throw }
      $drained += 1
    } finally {
      if ($nativeHandle -ne [IntPtr]::Zero) {
        [void][TalkingQuillProcessHandle]::CloseHandle($nativeHandle)
      }
    }
  }

  try {
    Move-Item -LiteralPath $root -Destination $recovery -ErrorAction Stop
    [Console]::Write("quarantined;attempt=$attempt;drained=$drained")
    exit 0
  } catch {
    $lastError = $_.Exception.Message -replace '[\r\n;]', ' '
    if ($attempt -lt $RetryCount) {
      Start-Sleep -Milliseconds $RetryDelayMilliseconds
    }
  }
}

# Identify every process currently mapped from the root, including foreign
# blockers that were intentionally not killed.
$blockers = @(Get-CimInstance Win32_Process | Where-Object {
  $null -ne $_.ExecutablePath -and
  $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
} | ForEach-Object { "pid=$($_.ProcessId),name=$($_.Name),path=$($_.ExecutablePath)" })
$identity = if ($blockers.Count -eq 0) { 'no-image-path-blocker-found' } else { $blockers -join '|' }
[Console]::Error.Write("quarantine-failed;attempts=$RetryCount;blockers=$identity;error=$lastError")
exit 70
