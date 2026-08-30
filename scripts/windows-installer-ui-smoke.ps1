param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [string]$Output = 'tmp/windows-installer-ui-smoke.json',
    [ValidateRange(5, 120)][int]$TimeoutSeconds = 30
)
$ErrorActionPreference = [Management.Automation.ActionPreference]::Stop
if (-not $IsWindows -and $env:OS -ne 'Windows_NT') { throw 'The installer UI smoke harness requires Windows.' }
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The disposable installer UI smoke harness must run elevated so it can enforce process cleanup.'
}
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$bytes = [IO.File]::ReadAllBytes($installerPath)
if ($bytes.Length -lt 256 -or [BitConverter]::ToUInt16($bytes, 0) -ne 0x5a4d) { throw 'Installer is not a PE executable.' }
$pe = [BitConverter]::ToUInt32($bytes, 0x3c)
if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x00004550) { throw 'Installer PE signature is invalid.' }
$subsystem = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)
if ($subsystem -ne 2) { throw "Installer subsystem is $subsystem, expected Windows GUI (2)." }
$programFilesTarget = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Talking Quill'
$programData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
$protectedBefore = @(Get-ChildItem -Force -LiteralPath $programData | Where-Object { $_.Name -match '^\.Talking Quill\.Installer-[0-9a-f]{32}$' } | ForEach-Object Name | Sort-Object)
$registryTargets = @(
    'HKLM:\Software\Talking Quill',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\com.talkingquill.app'
)
if ((Test-Path -LiteralPath $programFilesTarget) -or @($registryTargets | Where-Object { Test-Path -LiteralPath $_ }).Count -ne 0) {
    throw 'UI smoke requires a disposable host with no existing Talking Quill installation.'
}
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class TalkingQuillInstallerWindows {
  public sealed class Window { public IntPtr Handle; public uint ProcessId; public string Title; public string ClassName; public string ImagePath; }
  private delegate bool EnumProc(IntPtr window, IntPtr data);
  [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr data);
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder value, int size);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder value, int size);
  [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
  [DllImport("kernel32.dll", SetLastError=true)] private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] private static extern bool QueryFullProcessImageName(IntPtr process, int flags, StringBuilder name, ref int size);
  [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
  public static Window[] Find() {
    var found = new List<Window>();
    EnumWindows((window, data) => {
      if (!IsWindowVisible(window)) return true;
      var title = new StringBuilder(1024); GetWindowText(window, title, title.Capacity);
      if (title.ToString().IndexOf("Talking Quill", StringComparison.OrdinalIgnoreCase) < 0) return true;
      var name = new StringBuilder(256); GetClassName(window, name, name.Capacity);
      uint pid; GetWindowThreadProcessId(window, out pid);
      string imagePath = String.Empty;
      IntPtr process = OpenProcess(0x1000, false, pid);
      if (process != IntPtr.Zero) {
        try { var image = new StringBuilder(32768); int size = image.Capacity;
          if (QueryFullProcessImageName(process, 0, image, ref size)) imagePath = image.ToString(); }
        finally { CloseHandle(process); }
      }
      found.Add(new Window { Handle=window, ProcessId=pid, Title=title.ToString(), ClassName=name.ToString(), ImagePath=imagePath });
      return true;
    }, IntPtr.Zero);
    return found.ToArray();
  }
}
'@
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
$supervisor = [IO.Path]::GetFullPath((Join-Path (Get-Location) 'tmp/windows-installer-ui-supervisor.exe'))
$csc = Join-Path ([Environment]::GetFolderPath('Windows')) 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $csc /nologo "/out:$supervisor" (Join-Path (Get-Location) 'scripts/windows-job-object-supervisor.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the installer UI Job Object supervisor.' }
$quotedInstaller = '"' + $installerPath + '"'
$started = Start-Process -FilePath $supervisor -ArgumentList @(([string]($TimeoutSeconds * 1000)), $quotedInstaller, 'IA==') -PassThru
$observed = $null
try {
    while ([DateTime]::UtcNow -lt $deadline) {
        $windows = @([TalkingQuillInstallerWindows]::Find() | Where-Object { $_.ImagePath -eq $installerPath })
        if ($windows.Count -gt 0) { $observed = $windows[0]; break }
        Start-Sleep -Milliseconds 100
    }
    if ($null -eq $observed) { throw 'Timed out waiting for the interactive installer top-level window.' }
    [TalkingQuillInstallerWindows]::PostMessage($observed.Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    while ([DateTime]::UtcNow -lt $deadline) {
        $windows = @([TalkingQuillInstallerWindows]::Find() | Where-Object { $_.ImagePath -eq $installerPath })
        if ($windows.Count -eq 0 -and -not (Get-Process -Id $observed.ProcessId -ErrorAction SilentlyContinue)) { break }
        foreach ($window in $windows) {
            [TalkingQuillInstallerWindows]::PostMessage($window.Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
        }
        Start-Sleep -Milliseconds 100
    }
    $remainingWindows = @([TalkingQuillInstallerWindows]::Find() | Where-Object { $_.ImagePath -eq $installerPath })
    if ($remainingWindows.Count -ne 0 -or (Get-Process -Id $observed.ProcessId -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $observed.ProcessId -Force -ErrorAction SilentlyContinue
        throw 'Installer UI process did not exit within the cancellation bound.'
    }
    while (-not $started.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (-not $started.HasExited) { throw 'Installer Job Object did not drain within the cancellation bound.' }
    $sameImage = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $installerPath })
    if ($sameImage.Count -ne 0) { throw 'Installer executable processes remain after cancellation.' }
    $protectedAfter = @(Get-ChildItem -Force -LiteralPath $programData | Where-Object { $_.Name -match '^\.Talking Quill\.Installer-[0-9a-f]{32}$' } | ForEach-Object Name | Sort-Object)
    if (Compare-Object $protectedBefore $protectedAfter) { throw 'Protected bootstrap residue changed during UI smoke.' }
    if ((Test-Path -LiteralPath $programFilesTarget) -or @($registryTargets | Where-Object { Test-Path -LiteralPath $_ }).Count -ne 0) {
        throw 'Installer mutated machine installation state before UI cancellation.'
    }
    $evidence = [ordered]@{
        schemaVersion = 1
        installer = [IO.Path]::GetFileName($installerPath)
        subsystem = 'windows-gui'
        subsystemValue = $subsystem
        windowTitle = $observed.Title
        windowClass = $observed.ClassName
        processId = $observed.ProcessId
        silentMode = $false
        cancelledBeforeMutation = $true
        timeoutSeconds = $TimeoutSeconds
        observedAtUtc = [DateTime]::UtcNow.ToString('o')
    }
    $outputPath = [IO.Path]::GetFullPath($Output)
    $projectTmp = [IO.Path]::GetFullPath((Join-Path (Get-Location) 'tmp')) + [IO.Path]::DirectorySeparatorChar
    if (-not $outputPath.StartsWith($projectTmp, [StringComparison]::OrdinalIgnoreCase)) { throw 'Smoke evidence output must be under project tmp/.' }
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($outputPath)) | Out-Null
    [IO.File]::WriteAllText($outputPath, ($evidence | ConvertTo-Json) + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
    $evidence | ConvertTo-Json -Compress
} finally {
    if ($null -ne $started -and -not $started.HasExited) {
        Stop-Process -Id $started.Id -Force -ErrorAction SilentlyContinue
        $started.WaitForExit(10000) | Out-Null
    }
    if ($null -ne $observed) { Stop-Process -Id $observed.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $supervisor -Force -ErrorAction SilentlyContinue
}
