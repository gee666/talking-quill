[CmdletBinding()]
param(
    [switch]$Apply,
    [ValidateRange(1, 8760)]
    [int]$MinimumAgeHours = 24
)

$ErrorActionPreference = [Management.Automation.ActionPreference]::Stop
if ($env:OS -ne 'Windows_NT') { throw 'This command is supported only on Windows.' }
if ($Apply) {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Apply mode requires an elevated Administrator shell.'
    }
}

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class TalkingQuillCleanupNative
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Information
    {
        internal uint Attributes;
        internal System.Runtime.InteropServices.ComTypes.FILETIME Creation;
        internal System.Runtime.InteropServices.ComTypes.FILETIME Access;
        internal System.Runtime.InteropServices.ComTypes.FILETIME Write;
        internal uint Volume;
        internal uint SizeHigh;
        internal uint SizeLow;
        internal uint Links;
        internal uint IndexHigh;
        internal uint IndexLow;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateFile(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(IntPtr file, out Information information);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    public static string Identity(string path)
    {
        IntPtr file = CreateFile(path, 0x80, 7, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (file == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            Information value;
            if (!GetFileInformationByHandle(file, out value))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return value.Volume.ToString("x8") + ":" + value.IndexHigh.ToString("x8") +
                value.IndexLow.ToString("x8");
        }
        finally { CloseHandle(file); }
    }
}
'@

function Test-ExactAcl([string]$Path) {
    $acl = Get-Acl -LiteralPath $Path
    $allowed = @('S-1-5-18', 'S-1-5-32-544')
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    $rules = @($acl.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
    return $acl.AreAccessRulesProtected -and $owner -in $allowed -and $rules.Count -eq 2 -and
        @($rules | Where-Object {
            $_.AccessControlType -ne 'Allow' -or $_.IdentityReference.Value -notin $allowed -or
            $_.FileSystemRights -ne 'FullControl' -or
            $_.InheritanceFlags -ne 'ContainerInherit, ObjectInherit' -or
            $_.PropagationFlags -ne 'None'
        }).Count -eq 0
}

function Get-SafeTree([string]$Path) {
    $result = New-Object Collections.Generic.List[IO.FileSystemInfo]
    $pending = New-Object Collections.Generic.Stack[string]
    $pending.Push($Path)
    while ($pending.Count -ne 0) {
        $current = $pending.Pop()
        foreach ($item in @(Get-ChildItem -Force -LiteralPath $current)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'reparse content'
            }
            $result.Add($item)
            if ($item.PSIsContainer) { $pending.Push($item.FullName) }
        }
    }
    return $result
}

function Read-Manifest([string]$Leaf, [IO.FileSystemInfo[]]$Tree) {
    $files = @($Tree | Where-Object Name -eq '.talking-quill-bootstrap-manifest.json')
    if ($files.Count -eq 0) { return $null }
    if ($files.Count -ne 1 -or $files[0].DirectoryName -cne $Leaf -or $files[0].Length -gt 8192) {
        throw 'invalid manifest location or size'
    }
    $manifest = Get-Content -Raw -LiteralPath $files[0].FullName | ConvertFrom-Json
    if ($manifest.schemaVersion -ne 1 -or
        $manifest.kind -ne 'talking-quill-protected-bootstrap' -or
        $manifest.runId -cnotmatch '^[0-9a-f]{32}$' -or
        $manifest.leafName -cne [IO.Path]::GetFileName($Leaf) -or
        -not $manifest.leafName.EndsWith($manifest.runId, [StringComparison]::Ordinal) -or
        $manifest.identity -cnotmatch '^[0-9a-f]{8}:[0-9a-f]{16}$' -or
        $manifest.state -notin @('created', 'running', 'completed')) {
        throw 'invalid manifest fields'
    }
    return $manifest
}

function Test-AllowlistedContent([string]$Leaf, [string]$Kind, [IO.FileSystemInfo[]]$Tree) {
    foreach ($item in $Tree) {
        $relative = $item.FullName.Substring($Leaf.Length + 1)
        if ($relative -in @(
            '.talking-quill-bootstrap-manifest.json',
            '.talking-quill-bootstrap-manifest.pending',
            '.talking-quill-bootstrap-manifest.json.previous')) {
            if ($item.PSIsContainer) { return $false }
            continue
        }
        if ($Kind -eq 'harness') { return $false }
        if ($relative -match '^~ns[a-zA-Z0-9]+\.tmp$') {
            if (-not $item.PSIsContainer) { return $false }
            continue
        }
        if ($relative -match '^~ns[a-zA-Z0-9]+\.tmp\\Un_[a-zA-Z0-9]+\.exe$') {
            if ($item.PSIsContainer) { return $false }
            continue
        }
        return $false
    }
    return $true
}

function Test-Live([string]$Leaf, $Manifest) {
    if ($null -ne $Manifest) {
        foreach ($pair in @(
            @($Manifest.bootstrapPid, $Manifest.bootstrapStartUtc),
            @($Manifest.childPid, $Manifest.childStartUtc))) {
            if ($null -eq $pair[0] -or $null -eq $pair[1]) { continue }
            $process = Get-Process -Id ([int]$pair[0]) -ErrorAction SilentlyContinue
            if ($null -ne $process -and
                $process.StartTime.ToUniversalTime().ToString('o') -eq [string]$pair[1]) { return $true }
        }
    }
    foreach ($process in $script:runningProcesses) {
        if (-not [string]::IsNullOrEmpty($process.ExecutablePath) -and
            $process.ExecutablePath.StartsWith($Leaf + '\', [StringComparison]::OrdinalIgnoreCase)) {
            return $true
        }
    }
    return $false
}

function Remove-Bounded([string]$Path, [string]$Identity) {
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        if (-not (Test-Path -LiteralPath $Path)) { return $true }
        if ([TalkingQuillCleanupNative]::Identity($Path) -cne $Identity) { return $false }
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $Path) { Start-Sleep -Milliseconds 250 }
    }
    return -not (Test-Path -LiteralPath $Path)
}

$programData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData).TrimEnd('\')
$root = Get-Item -Force -LiteralPath $programData
if (-not $root.PSIsContainer -or ($root.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'Native ProgramData is not a plain directory.'
}
$cutoff = [DateTime]::UtcNow.AddHours(-$MinimumAgeHours)
$script:runningProcesses = @(Get-CimInstance Win32_Process -ErrorAction Stop)
$failed = $false
foreach ($leaf in @(Get-ChildItem -Force -LiteralPath $programData -Directory)) {
    $match = [regex]::Match($leaf.Name, '^\.Talking Quill\.(Installer|Harness)-([0-9a-f]{32})$')
    if (-not $match.Success) { continue }
    $status = 'eligible'
    $reason = $null
    $identity = $null
    try {
        if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($leaf.FullName)) -cne $programData) {
            throw 'not a native ProgramData direct child'
        }
        if (($leaf.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'reparse leaf' }
        if (-not (Test-ExactAcl $leaf.FullName)) { throw 'owner or DACL mismatch' }
        $identity = [TalkingQuillCleanupNative]::Identity($leaf.FullName)
        $tree = @(Get-SafeTree $leaf.FullName)
        $manifest = Read-Manifest $leaf.FullName $tree
        if ($null -ne $manifest -and $manifest.identity -cne $identity) { throw 'manifest identity mismatch' }
        $kind = $match.Groups[1].Value.ToLowerInvariant()
        if (-not (Test-AllowlistedContent $leaf.FullName $kind $tree)) { throw 'unknown content' }
        if ($leaf.LastWriteTimeUtc -gt $cutoff -or $leaf.CreationTimeUtc -gt $cutoff) {
            $status = 'recent'
        } elseif (Test-Live $leaf.FullName $manifest) {
            $status = 'live'
        }
    } catch {
        $status = 'refused'
        $reason = $_.Exception.Message
    }

    if ($status -eq 'eligible' -and $Apply) {
        if (Remove-Bounded $leaf.FullName $identity) { $status = 'removed' }
        else { $status = 'remove-failed'; $failed = $true }
    }
    [pscustomobject]@{ path = $leaf.FullName; status = $status; reason = $reason }
}
if ($failed) { exit 1 }
