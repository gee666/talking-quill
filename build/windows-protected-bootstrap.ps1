$ErrorActionPreference = [Management.Automation.ActionPreference]::Stop
$inheritedTemp = [Environment]::GetEnvironmentVariable('TEMP')
$inheritedTmp = [Environment]::GetEnvironmentVariable('TMP')
$programData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData).TrimEnd('\')
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
$bytes = New-Object byte[] 16
try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
$runId = -join ($bytes | ForEach-Object { $_.ToString('x2') })
$leaf = Join-Path $programData ('.Talking Quill.Installer-' + $runId)
$leafIdentity = $null
$directoryAcl = New-Object Security.AccessControl.DirectorySecurity
$directoryAcl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
$directoryAcl.SetAccessRuleProtection($true, $false)
foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
    $identity = New-Object Security.Principal.SecurityIdentifier($sid)
    $directoryAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
        $identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
}
$leafDirectory = [IO.Directory]::CreateDirectory($leaf, $directoryAcl)
try {
[Environment]::SetEnvironmentVariable('TEMP', $leaf, 'Process')
[Environment]::SetEnvironmentVariable('TMP', $leaf, 'Process')
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class TalkingQuillProtectedBootstrapNative
{
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessBasicInformation
    {
        internal IntPtr Reserved1;
        internal IntPtr PebBaseAddress;
        internal IntPtr Reserved2_0;
        internal IntPtr Reserved2_1;
        internal IntPtr UniqueProcessId;
        internal IntPtr ParentProcessId;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct UnicodeString
    {
        internal ushort Length;
        internal ushort MaximumLength;
        internal IntPtr Buffer;
    }

    [DllImport("ntdll.dll")]
    private static extern int NtQueryInformationProcess(
        IntPtr process,
        int informationClass,
        ref ProcessBasicInformation information,
        int informationLength,
        out int returnLength);

    [DllImport("ntdll.dll")]
    private static extern int NtQueryInformationProcess(
        IntPtr process,
        int informationClass,
        IntPtr information,
        int informationLength,
        out int returnLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inheritHandle, uint processId);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateFile(
        string name, uint access, uint share, IntPtr security, uint creation,
        uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        IntPtr file, out ByHandleFileInformation information);

    [StructLayout(LayoutKind.Sequential)]
    private struct ByHandleFileInformation
    {
        internal uint FileAttributes;
        internal System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        internal System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        internal System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        internal uint VolumeSerialNumber;
        internal uint FileSizeHigh;
        internal uint FileSizeLow;
        internal uint NumberOfLinks;
        internal uint FileIndexHigh;
        internal uint FileIndexLow;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool QueryFullProcessImageName(
        IntPtr process,
        int flags,
        StringBuilder executableName,
        ref int size);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CommandLineToArgvW(string commandLine, out int argumentCount);

    [DllImport("kernel32.dll")]
    private static extern IntPtr LocalFree(IntPtr memory);

    public static string[] ReadWaitingParent()
    {
        ProcessBasicInformation basic = new ProcessBasicInformation();
        int returned;
        int status = NtQueryInformationProcess(
            System.Diagnostics.Process.GetCurrentProcess().Handle,
            0,
            ref basic,
            Marshal.SizeOf(typeof(ProcessBasicInformation)),
            out returned);
        if (status != 0 || basic.ParentProcessId == IntPtr.Zero)
            throw new InvalidOperationException("The waiting parent process is unavailable.");

        IntPtr parent = OpenProcess(0x1000, false, unchecked((uint)basic.ParentProcessId.ToInt64()));
        if (parent == IntPtr.Zero)
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            StringBuilder image = new StringBuilder(32768);
            int imageLength = image.Capacity;
            if (!QueryFullProcessImageName(parent, 0, image, ref imageLength))
                throw new Win32Exception(Marshal.GetLastWin32Error());

            int required = 0;
            NtQueryInformationProcess(parent, 60, IntPtr.Zero, 0, out required);
            if (required <= Marshal.SizeOf(typeof(UnicodeString)) || required > 131072)
                throw new InvalidOperationException("The waiting parent command line is unavailable.");
            IntPtr commandBuffer = Marshal.AllocHGlobal(required);
            try
            {
                status = NtQueryInformationProcess(parent, 60, commandBuffer, required, out returned);
                if (status != 0)
                    throw new InvalidOperationException("The waiting parent command line query failed.");
                UnicodeString nativeCommand = (UnicodeString)Marshal.PtrToStructure(
                    commandBuffer,
                    typeof(UnicodeString));
                if (nativeCommand.Buffer == IntPtr.Zero || nativeCommand.Length == 0 ||
                    (nativeCommand.Length & 1) != 0 || nativeCommand.Length > required)
                    throw new InvalidOperationException("The waiting parent command line is malformed.");
                string commandLine = Marshal.PtrToStringUni(
                    nativeCommand.Buffer,
                    nativeCommand.Length / 2);
                int nsisTailOffset = commandLine.LastIndexOf(" _?=", StringComparison.Ordinal);
                string nsisTail = String.Empty;
                if (nsisTailOffset >= 0)
                {
                    nsisTail = commandLine.Substring(nsisTailOffset + 1);
                    commandLine = commandLine.Substring(0, nsisTailOffset);
                }
                int count;
                IntPtr argv = CommandLineToArgvW(commandLine, out count);
                if (argv == IntPtr.Zero || count < 1 || count > 4096)
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                try
                {
                    string[] result = new string[count + 2];
                    result[0] = image.ToString();
                    result[1] = nsisTail;
                    for (int index = 0; index < count; index++)
                    {
                        IntPtr value = Marshal.ReadIntPtr(argv, index * IntPtr.Size);
                        result[index + 2] = Marshal.PtrToStringUni(value);
                    }
                    return result;
                }
                finally
                {
                    LocalFree(argv);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(commandBuffer);
            }
        }
        finally
        {
            CloseHandle(parent);
        }
    }

    public static string GetPathIdentity(string path)
    {
        IntPtr file = CreateFile(path, 0x80, 7, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (file == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            ByHandleFileInformation information;
            if (!GetFileInformationByHandle(file, out information))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return information.VolumeSerialNumber.ToString("x8") + ":" +
                information.FileIndexHigh.ToString("x8") +
                information.FileIndexLow.ToString("x8");
        }
        finally { CloseHandle(file); }
    }

    public static string JoinArguments(string[] arguments)
    {
        StringBuilder result = new StringBuilder();
        foreach (string argument in arguments)
        {
            if (result.Length != 0) result.Append(' ');
            if (argument.Length != 0 && argument.IndexOfAny(new char[] { ' ', '\t', '\"' }) < 0)
            {
                result.Append(argument);
                continue;
            }
            result.Append('\"');
            int slashes = 0;
            foreach (char character in argument)
            {
                if (character == '\\')
                {
                    slashes++;
                }
                else if (character == '\"')
                {
                    result.Append('\\', slashes * 2 + 1);
                    result.Append('\"');
                    slashes = 0;
                }
                else
                {
                    result.Append('\\', slashes);
                    result.Append(character);
                    slashes = 0;
                }
            }
            result.Append('\\', slashes * 2);
            result.Append('\"');
        }
        return result.ToString();
    }
}
'@

function Test-OwnedLeaf([string]$path, [string]$identity) {
    try {
        $fullPath = [IO.Path]::GetFullPath($path).TrimEnd('\')
        if ([IO.Path]::GetDirectoryName($fullPath) -cne $programData -or
            [IO.Path]::GetFileName($fullPath) -cnotmatch '^\.Talking Quill\.Installer-[0-9a-f]{32}$') { return $false }
        $item = Get-Item -Force -LiteralPath $fullPath
        if (-not $item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { return $false }
        if ([TalkingQuillProtectedBootstrapNative]::GetPathIdentity($fullPath) -cne $identity) {
            return $false
        }
        $pending = New-Object Collections.Generic.Stack[string]
        $pending.Push($fullPath)
        while ($pending.Count -ne 0) {
            foreach ($childItem in @(Get-ChildItem -Force -LiteralPath $pending.Pop())) {
                if (($childItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    return $false
                }
                if ($childItem.PSIsContainer) { $pending.Push($childItem.FullName) }
            }
        }
        $acl = Get-Acl -LiteralPath $fullPath
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
    } catch { return $false }
}

function Write-LeafManifest([string]$state, $childProcess) {
    $manifest = [ordered]@{
        schemaVersion = 1
        kind = 'talking-quill-protected-bootstrap'
        runId = $runId
        leafName = [IO.Path]::GetFileName($leaf)
        identity = $leafIdentity
        createdUtc = $script:createdUtc
        bootstrapPid = $PID
        bootstrapStartUtc = (Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o')
        childPid = if ($null -eq $childProcess) { $null } else { $childProcess.Id }
        childStartUtc = if ($null -eq $childProcess) { $null } else { $childProcess.StartTime.ToUniversalTime().ToString('o') }
        state = $state
    }
    $pending = Join-Path $leaf '.talking-quill-bootstrap-manifest.pending'
    $final = Join-Path $leaf '.talking-quill-bootstrap-manifest.json'
    $bytes = [Text.Encoding]::UTF8.GetBytes(($manifest | ConvertTo-Json -Compress))
    $stream = New-Object IO.FileStream($pending, 'Create', 'Write', 'None', 4096, 'WriteThrough')
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    if ([IO.File]::Exists($final)) {
        $previous = $final + '.previous'
        [IO.File]::Replace($pending, $final, $previous)
        [IO.File]::Delete($previous)
    } else { [IO.File]::Move($pending, $final) }
}

$leafIdentity = [TalkingQuillProtectedBootstrapNative]::GetPathIdentity($leaf)
$script:createdUtc = [DateTime]::UtcNow.ToString('o')
Write-LeafManifest 'created' $null

$parent = [TalkingQuillProtectedBootstrapNative]::ReadWaitingParent()
$parentExecutable = $parent[0]
$nsisTail = $parent[1]
$parentArguments = @($parent[3..($parent.Length - 1)])
$publicArguments = [Collections.Generic.List[string]]::new()
$protectedTemp = $null
$elevatedMarker = $false
foreach ($argument in $parentArguments) {
    if ($argument.Equals('/TQELEVATEDBOOTSTRAP=1', [StringComparison]::OrdinalIgnoreCase)) {
        if ($elevatedMarker) { throw 'The elevated bootstrap marker is duplicated.' }
        $elevatedMarker = $true
        continue
    }
    if ($argument.StartsWith('/TQELEVATEDBOOTSTRAP=', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The elevated bootstrap marker is malformed.'
    }
    if ($argument.StartsWith('/TQPROTECTEDTEMP=', [StringComparison]::OrdinalIgnoreCase)) {
        if ($null -ne $protectedTemp) { throw 'The protected TEMP marker is duplicated.' }
        $protectedTemp = $argument.Substring('/TQPROTECTEDTEMP='.Length)
        if ([string]::IsNullOrEmpty($protectedTemp)) { throw 'The protected TEMP marker is empty.' }
        continue
    }
    $publicArguments.Add($argument)
}

if (-not [string]::IsNullOrEmpty($nsisTail)) {
    $nativeProgramFilesKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
        [Microsoft.Win32.RegistryHive]::LocalMachine,
        [Microsoft.Win32.RegistryView]::Registry64).OpenSubKey(
            'SOFTWARE\Microsoft\Windows\CurrentVersion', $false)
    if ($null -eq $nativeProgramFilesKey -or $nsisTail.Length -le 3) { exit 78 }
    try { $nativeProgramFiles = [string]$nativeProgramFilesKey.GetValue('ProgramFilesDir') }
    finally { $nativeProgramFilesKey.Dispose() }
    $expectedNsisRoot = Join-Path $nativeProgramFiles 'Talking Quill'
    $nsisRoot = [IO.Path]::GetFullPath($nsisTail.Substring(3)).TrimEnd('\')
    if (-not $nsisRoot.Equals($expectedNsisRoot, [StringComparison]::OrdinalIgnoreCase)) { exit 78 }
    $nsisRootItem = Get-Item -Force -LiteralPath $nsisRoot
    if (-not $nsisRootItem.PSIsContainer -or
        ($nsisRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { exit 78 }
}
if ($null -ne $protectedTemp) {
    $path = [IO.Path]::GetFullPath($protectedTemp)
    if ([IO.Path]::GetDirectoryName($path) -cne $programData -or
        [IO.Path]::GetFileName($path) -cnotmatch '^\.Talking Quill\.Installer-[0-9a-f]{32}$') {
        exit 78
    }
    if (-not $path.Equals($inheritedTemp, [StringComparison]::Ordinal) -or
        -not $path.Equals($inheritedTmp, [StringComparison]::Ordinal)) {
        exit 78
    }
    $item = Get-Item -Force -LiteralPath $path
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { exit 78 }
    $acl = Get-Acl -LiteralPath $path
    $allowed = @('S-1-5-18', 'S-1-5-32-544')
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    $rules = @($acl.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
    if (-not $acl.AreAccessRulesProtected -or $owner -notin $allowed -or $rules.Count -ne 2 -or
        @($rules | Where-Object {
            $_.AccessControlType -ne 'Allow' -or $_.IdentityReference.Value -notin $allowed -or
            $_.FileSystemRights -ne 'FullControl' -or
            $_.InheritanceFlags -ne 'ContainerInherit, ObjectInherit' -or $_.PropagationFlags -ne 'None'
        }).Count -ne 0) {
        exit 78
    }
    exit 0
}

if (-not $elevatedMarker) { exit 78 }
$childArguments = @($publicArguments)
$childArguments += '/TQELEVATEDBOOTSTRAP=1'
$start = New-Object Diagnostics.ProcessStartInfo
$start.FileName = $parentExecutable
$start.Arguments = [TalkingQuillProtectedBootstrapNative]::JoinArguments($childArguments) +
    ' /TQPROTECTEDTEMP=' + [TalkingQuillProtectedBootstrapNative]::JoinArguments(@($leaf))
if (-not [string]::IsNullOrEmpty($nsisTail)) { $start.Arguments += ' ' + $nsisTail }
$start.UseShellExecute = $false
$start.EnvironmentVariables['TEMP'] = $leaf
$start.EnvironmentVariables['TMP'] = $leaf
$child = [Diagnostics.Process]::Start($start)
Write-LeafManifest 'running' $child
$child.WaitForExit()
$childExitCode = $child.ExitCode
Write-LeafManifest 'completed' $child
exit $childExitCode
} catch {
    [Console]::Error.WriteLine('Protected bootstrap failed: ' + $_.Exception.Message)
    exit 78
} finally {
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        if (-not (Test-Path -LiteralPath $leaf)) { break }
        if ($null -ne $leafIdentity) {
            if (-not (Test-OwnedLeaf $leaf $leafIdentity)) {
                [Console]::Error.WriteLine('Protected bootstrap refused to remove a leaf whose validated identity changed or contains a reparse point.')
                break
            }
            Remove-Item -LiteralPath $leaf -Recurse -Force -ErrorAction SilentlyContinue
        } else {
            try {
                $fallbackPath = [IO.Path]::GetFullPath($leafDirectory.FullName).TrimEnd('\')
                $fallbackItem = Get-Item -Force -LiteralPath $fallbackPath
                if ([IO.Path]::GetDirectoryName($fallbackPath) -cne $programData -or
                    [IO.Path]::GetFileName($fallbackPath) -cnotmatch '^\.Talking Quill\.Installer-[0-9a-f]{32}$' -or
                    ($fallbackItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
                    @(Get-ChildItem -Force -LiteralPath $fallbackPath).Count -ne 0) { break }
                [IO.Directory]::Delete($fallbackPath, $false)
            } catch { }
        }
        if (Test-Path -LiteralPath $leaf) { Start-Sleep -Milliseconds 250 }
    }
    if (Test-Path -LiteralPath $leaf) {
        [Console]::Error.WriteLine('Protected bootstrap could not remove its ProgramData leaf after bounded retries: ' + $leaf)
    }
}
