[CmdletBinding()]
param(
    [switch]$Apply,
    [ValidateRange(1, 8760)]
    [int]$MinimumAgeHours = 24,
    [ValidatePattern('^[0-9a-f]{32}$')]
    [string]$RunId = ''
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
using System.Text;

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

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessBasicInformation
    {
        internal IntPtr Reserved1;
        internal IntPtr Peb;
        internal IntPtr Reserved2;
        internal IntPtr Reserved3;
        internal IntPtr ProcessId;
        internal IntPtr ParentProcessId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateFile(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(IntPtr file, out Information information);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inheritHandle, uint processId);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool ReadProcessMemory(IntPtr process, IntPtr address, byte[] buffer,
        int size, out IntPtr bytesRead);
    [DllImport("ntdll.dll")]
    private static extern int NtQueryInformationProcess(IntPtr process, int informationClass,
        ref ProcessBasicInformation information, int informationLength, out int returnLength);
    [DllImport("ntdll.dll")]
    private static extern int NtQueryInformationProcess(IntPtr process, int informationClass,
        out IntPtr information, int informationLength, out int returnLength);

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

    // Returns 1 for a reference, 0 for no reference, and -1 when the process cannot be inspected.
    public static int EnvironmentReferences(uint processId, string expected)
    {
        IntPtr process = OpenProcess(0x0410, false, processId);
        if (process == IntPtr.Zero) return -1;
        try
        {
            IntPtr wowPeb;
            int returned;
            int wowStatus = NtQueryInformationProcess(process, 26, out wowPeb, IntPtr.Size, out returned);
            bool wow64 = wowStatus == 0 && wowPeb != IntPtr.Zero;
            IntPtr peb;
            if (wow64) peb = wowPeb;
            else
            {
                ProcessBasicInformation basic = new ProcessBasicInformation();
                int status = NtQueryInformationProcess(process, 0, ref basic,
                    Marshal.SizeOf(typeof(ProcessBasicInformation)), out returned);
                if (status != 0 || basic.Peb == IntPtr.Zero) return -1;
                peb = basic.Peb;
            }

            IntPtr parameters = ReadPointer(process, Add(peb, wow64 ? 0x10 : 0x20), wow64);
            if (parameters == IntPtr.Zero) return -1;
            IntPtr environment = ReadPointer(process, Add(parameters, wow64 ? 0x48 : 0x80), wow64);
            if (environment == IntPtr.Zero) return 0;

            Decoder decoder = Encoding.Unicode.GetDecoder();
            StringBuilder text = new StringBuilder();
            byte[] buffer = new byte[4096];
            char[] characters = new char[4096];
            for (int offset = 0; offset < 1024 * 1024; offset += buffer.Length)
            {
                IntPtr read;
                if (!ReadProcessMemory(process, Add(environment, offset), buffer, buffer.Length, out read) ||
                    read.ToInt64() <= 0) return -1;
                int count = decoder.GetChars(buffer, 0, (int)read.ToInt64(), characters, 0, false);
                text.Append(characters, 0, count);
                string value = text.ToString();
                if (value.IndexOf(expected, StringComparison.OrdinalIgnoreCase) >= 0) return 1;
                if (value.IndexOf("\0\0", StringComparison.Ordinal) >= 0) return 0;
                if (text.Length > expected.Length + 4)
                    text.Remove(0, text.Length - expected.Length - 4);
            }
            return -1;
        }
        catch { return -1; }
        finally { CloseHandle(process); }
    }

    private static IntPtr ReadPointer(IntPtr process, IntPtr address, bool pointer32)
    {
        byte[] value = new byte[pointer32 ? 4 : 8];
        IntPtr read;
        if (!ReadProcessMemory(process, address, value, value.Length, out read) ||
            read.ToInt64() != value.Length) return IntPtr.Zero;
        return pointer32
            ? new IntPtr(unchecked((long)BitConverter.ToUInt32(value, 0)))
            : new IntPtr(BitConverter.ToInt64(value, 0));
    }

    private static IntPtr Add(IntPtr value, int offset)
    {
        return new IntPtr(value.ToInt64() + offset);
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
        foreach ($item in @(Get-ChildItem -Force -LiteralPath $pending.Pop())) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'reparse content'
            }
            $result.Add($item)
            if ($item.PSIsContainer) { $pending.Push($item.FullName) }
        }
    }
    return $result
}

function ConvertTo-ValidatedManifest(
    [IO.FileInfo]$File,
    [string]$ExpectedLeafName,
    [string]$Identity
) {
    if ($File.Length -lt 2 -or $File.Length -gt 8192) { throw 'invalid manifest size' }
    $raw = Get-Content -Raw -LiteralPath $File.FullName
    $manifest = $raw | ConvertFrom-Json
    if ($manifest.schemaVersion -ne 1 -or
        $manifest.kind -ne 'talking-quill-protected-bootstrap' -or
        $manifest.runId -cnotmatch '^[0-9a-f]{32}$' -or
        $manifest.leafName -cne $ExpectedLeafName -or
        -not $manifest.leafName.EndsWith($manifest.runId, [StringComparison]::Ordinal) -or
        $manifest.identity -cne $Identity -or
        $manifest.state -notin @('created', 'running', 'completed')) {
        throw 'invalid manifest fields'
    }
    $created = [DateTimeOffset]::MinValue
    $bootstrapStart = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse([string]$manifest.createdUtc, [ref]$created) -or
        -not [DateTimeOffset]::TryParse([string]$manifest.bootstrapStartUtc, [ref]$bootstrapStart) -or
        $created.Offset -ne [TimeSpan]::Zero -or $bootstrapStart.Offset -ne [TimeSpan]::Zero -or
        $created.UtcDateTime -gt [DateTime]::UtcNow.AddMinutes(1)) { throw 'invalid manifest timestamps' }
    foreach ($property in @('bootstrapPid', 'childPid')) {
        $value = $manifest.$property
        if ($null -ne $value -and ([long]$value -lt 1 -or [long]$value -gt [UInt32]::MaxValue)) {
            throw 'invalid manifest PID'
        }
    }
    if ($null -ne $manifest.childPid) {
        $childStart = [DateTimeOffset]::MinValue
        if (-not [DateTimeOffset]::TryParse([string]$manifest.childStartUtc, [ref]$childStart) -or
            $childStart.Offset -ne [TimeSpan]::Zero) { throw 'invalid child timestamp' }
    }
    return [pscustomobject]@{ Value = $manifest; Raw = $raw }
}

function Get-AllowlistedInventory(
    [string]$Leaf,
    [string]$Kind,
    [IO.FileSystemInfo[]]$Tree,
    [string]$Identity,
    [string]$ExpectedLeafName = ''
) {
    if ([string]::IsNullOrEmpty($ExpectedLeafName)) {
        $ExpectedLeafName = [IO.Path]::GetFileName($Leaf)
    }
    $manifest = $null
    $inventory = New-Object Collections.Generic.List[string]
    foreach ($item in @($Tree | Sort-Object FullName)) {
        $relative = $item.FullName.Substring($Leaf.Length + 1)
        $allowedManifest = $relative -in @(
            '.talking-quill-bootstrap-manifest.json',
            '.talking-quill-bootstrap-manifest.pending',
            '.talking-quill-bootstrap-manifest.json.previous')
        if ($allowedManifest) {
            if ($item.PSIsContainer) { throw 'manifest is a directory' }
            $validated = ConvertTo-ValidatedManifest $item $ExpectedLeafName $Identity
            if ($relative -eq '.talking-quill-bootstrap-manifest.json') {
                $manifest = $validated.Value
            }
            $hash = [Convert]::ToBase64String(
                [Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($validated.Raw)))
            $inventory.Add($relative + '|manifest|' + $item.CreationTimeUtc.Ticks + '|' +
                $item.LastWriteTimeUtc.Ticks + '|' + $hash)
            continue
        }
        if ($Kind -eq 'installer' -and $relative -match '^~ns[a-zA-Z0-9]+\.tmp$' -and $item.PSIsContainer) {
            $inventory.Add($relative + '|directory|' + $item.CreationTimeUtc.Ticks)
            continue
        }
        if ($Kind -eq 'installer' -and
            $relative -match '^~ns[a-zA-Z0-9]+\.tmp\\Un_[a-zA-Z0-9]+\.exe$' -and
            -not $item.PSIsContainer) {
            $fileHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
            $inventory.Add($relative + '|file|' + $item.Length + '|' +
                $item.CreationTimeUtc.Ticks + '|' + $item.LastWriteTimeUtc.Ticks + '|' + $fileHash)
            continue
        }
        throw 'unknown content'
    }
    return [pscustomobject]@{ Manifest = $manifest; Signature = ($inventory -join "`n") }
}

function Test-ManifestProcess([object[]]$Processes, $Manifest) {
    if ($null -eq $Manifest) { return $false }
    foreach ($pair in @(
        @($Manifest.bootstrapPid, $Manifest.bootstrapStartUtc),
        @($Manifest.childPid, $Manifest.childStartUtc))) {
        if ($null -eq $pair[0] -or $null -eq $pair[1]) { continue }
        $process = Get-Process -Id ([int]$pair[0]) -ErrorAction SilentlyContinue
        if ($null -ne $process -and
            $process.StartTime.ToUniversalTime().ToString('o') -eq [string]$pair[1]) { return $true }
    }
    return $false
}

function Test-LiveReference([string]$Leaf, $Manifest) {
    $processes = @(Get-CimInstance Win32_Process -ErrorAction Stop)
    if (Test-ManifestProcess $processes $Manifest) { return $true }
    $associated = New-Object 'Collections.Generic.HashSet[uint32]'
    $uninspectableCurrentSession = $false
    foreach ($process in $processes) {
        $image = [string]$process.ExecutablePath
        $command = [string]$process.CommandLine
        $references = (-not [string]::IsNullOrEmpty($image) -and
                $image.StartsWith($Leaf + '\', [StringComparison]::OrdinalIgnoreCase)) -or
            (-not [string]::IsNullOrEmpty($command) -and
                $command.IndexOf($Leaf, [StringComparison]::OrdinalIgnoreCase) -ge 0)
        $environment = [TalkingQuillCleanupNative]::EnvironmentReferences([uint32]$process.ProcessId, $Leaf)
        if ($environment -eq 1) { $references = $true }
        if ($environment -eq -1 -and [uint32]$process.SessionId -eq $script:currentSessionId -and
            [string]$process.Name -match '(?i)(installer|uninstall|powershell|pwsh|msiexec|ns.*tmp|job-object-supervisor)') {
            $uninspectableCurrentSession = $true
        }
        if ($references) { [void]$associated.Add([uint32]$process.ProcessId) }
    }
    $changed = $true
    while ($changed) {
        $changed = $false
        foreach ($process in $processes) {
            if ($associated.Contains([uint32]$process.ParentProcessId) -and
                $associated.Add([uint32]$process.ProcessId)) { $changed = $true }
        }
    }
    if ($associated.Count -ne 0) { return $true }
    if ($uninspectableCurrentSession) {
        throw 'a current-session process environment could not be inspected'
    }
    return $false
}

function Get-CandidateValidation(
    [string]$Path,
    [string]$ExpectedIdentity = '',
    [string]$ExpectedSignature = '',
    [switch]$AllowRemovalProgress
) {
    $fullPath = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if ([IO.Path]::GetDirectoryName($fullPath) -cne $script:programData) {
        throw 'not a native ProgramData direct child'
    }
    $match = [regex]::Match([IO.Path]::GetFileName($fullPath),
        '^\.Talking Quill\.(Installer|Harness)-([0-9a-f]{32})$')
    if (-not $match.Success) { throw 'invalid leaf name' }
    $leaf = Get-Item -Force -LiteralPath $fullPath
    if (-not $leaf.PSIsContainer -or
        ($leaf.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'reparse leaf' }
    if (-not (Test-ExactAcl $fullPath)) { throw 'owner or DACL mismatch' }
    $identity = [TalkingQuillCleanupNative]::Identity($fullPath)
    if (-not [string]::IsNullOrEmpty($ExpectedIdentity) -and $identity -cne $ExpectedIdentity) {
        throw 'leaf identity changed'
    }
    $tree = @(Get-SafeTree $fullPath)
    $content = Get-AllowlistedInventory $fullPath $match.Groups[1].Value.ToLowerInvariant() $tree $identity
    $signature = $leaf.CreationTimeUtc.Ticks.ToString() + "`n" + $content.Signature
    if (-not [string]::IsNullOrEmpty($ExpectedSignature)) {
        if (-not $AllowRemovalProgress -and $signature -cne $ExpectedSignature) {
            throw 'leaf timestamps or content changed'
        }
        if ($AllowRemovalProgress) {
            $expectedLines = New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
            foreach ($line in @($ExpectedSignature -split "`n")) {
                if (-not [string]::IsNullOrEmpty($line)) { [void]$expectedLines.Add($line) }
            }
            foreach ($line in @($signature -split "`n")) {
                if (-not [string]::IsNullOrEmpty($line) -and -not $expectedLines.Contains($line)) {
                    throw 'leaf content was added or replaced during removal'
                }
            }
        }
    }
    if ($leaf.CreationTimeUtc -gt $script:cutoff -or $leaf.LastWriteTimeUtc -gt $script:cutoff) {
        throw 'recent leaf'
    }
    if (Test-LiveReference $fullPath $content.Manifest) { throw 'live leaf reference' }
    return [pscustomobject]@{
        Path = $fullPath
        Identity = $identity
        Signature = $signature
        LeafName = $leaf.Name
        Kind = $match.Groups[1].Value.ToLowerInvariant()
        CreationTicks = $leaf.CreationTimeUtc.Ticks
        Manifest = $content.Manifest
    }
}

function Remove-Revalidated([object]$Candidate) {
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        if (-not (Test-Path -LiteralPath $Candidate.Path)) { return $true }
        try {
            [void](Get-CandidateValidation $Candidate.Path $Candidate.Identity $Candidate.Signature `
                -AllowRemovalProgress:($attempt -ne 0))
        } catch { return $false }
        Remove-Item -LiteralPath $Candidate.Path -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $Candidate.Path) { Start-Sleep -Milliseconds 250 }
    }
    return -not (Test-Path -LiteralPath $Candidate.Path)
}

$script:programData = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::CommonApplicationData).TrimEnd('\')
$root = Get-Item -Force -LiteralPath $script:programData
if (-not $root.PSIsContainer -or ($root.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'Native ProgramData is not a plain directory.'
}
$script:cutoff = [DateTime]::UtcNow.AddHours(-$MinimumAgeHours)
$script:currentSessionId = [uint32](Get-Process -Id $PID).SessionId
$failed = $false
foreach ($leaf in @(Get-ChildItem -Force -LiteralPath $script:programData -Directory)) {
    if (-not [string]::IsNullOrEmpty($RunId) -and
        -not $leaf.Name.EndsWith('-' + $RunId, [StringComparison]::Ordinal)) { continue }
    if ($leaf.Name -cnotmatch '^\.Talking Quill\.(Installer|Harness)-[0-9a-f]{32}$') { continue }
    $status = 'eligible'
    $reason = $null
    $candidate = $null
    try {
        $candidate = Get-CandidateValidation $leaf.FullName
    } catch {
        $message = $_.Exception.Message
        if ($message -eq 'recent leaf') { $status = 'recent' }
        elseif ($message -eq 'live leaf reference') { $status = 'live' }
        else { $status = 'refused' }
        $reason = $message
    }
    if ($status -eq 'eligible' -and $Apply) {
        if (Remove-Revalidated $candidate) { $status = 'removed' }
        else { $status = 'remove-failed'; $failed = $true }
    }
    [pscustomobject]@{ path = $leaf.FullName; status = $status; reason = $reason }
}
if ($failed) { exit 1 }
