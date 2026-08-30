[CmdletBinding()]
param(
  [switch]$SelfTest,
  [switch]$ElevatedWorker,
  [string]$ElevatedPipeName,
  [string]$ElevatedCapability,
  [string]$OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:CollectorVersion = '8'
$script:CurrentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$script:SidHash = ([BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($script:CurrentSid))).Replace('-', '').ToLowerInvariant()).Substring(0, 16)
$script:UserProfile = [Environment]::GetFolderPath('UserProfile')
$script:UserName = [Environment]::UserName
$script:RelevantProcessPattern = '^(Talking Quill|talking-quill-(helper|keyboard-owner))\.exe$'
$script:AllowedDiagnosticEvents = @('application.started', 'application.stopping', 'helper.runtime.snapshot', 'helper.readiness.changed', 'helper.startup.failure', 'helper.operational.failure')

function Get-ShortHash([AllowNull()][string]$Value) {
  if ($null -eq $Value) { $Value = '' }
  $sha = [Security.Cryptography.SHA256]::Create()
  try { return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value))).Replace('-', '').ToLowerInvariant()).Substring(0, 16) }
  finally { $sha.Dispose() }
}

function Get-SafeProperty {
  param(
    [AllowNull()]$InputObject,
    [Parameter(Mandatory = $true)][string]$Name,
    [AllowNull()]$Default = $null
  )
  if ($null -eq $InputObject) { return $Default }
  try {
    if ($InputObject -is [Collections.IDictionary]) {
      if ($InputObject.Contains($Name)) { return $InputObject[$Name] }
      return $Default
    }
    $property = $InputObject.PSObject.Properties[$Name]
    if ($null -eq $property) { return $Default }
    return $property.Value
  } catch { return $Default }
}

function ConvertTo-SafeInt {
  param([AllowNull()]$Value, [int]$Default = 0)
  $number = 0
  if ($null -ne $Value -and [int]::TryParse([string]$Value, [ref]$number)) { return $number }
  return $Default
}

function ConvertTo-SafeUtcText([AllowNull()]$Value) {
  if ($null -eq $Value) { return $null }
  try { return ([DateTime]$Value).ToUniversalTime().ToString('o') } catch { return $null }
}

function ConvertTo-ValidatedDiagnosticTimestamp([AllowNull()]$Value) {
  $parsed = [DateTimeOffset]::MinValue
  if ($Value -is [byte] -or $Value -is [int16] -or $Value -is [int32] -or $Value -is [int64] -or $Value -is [decimal] -or $Value -is [double]) {
    $milliseconds = [int64]0
    if (-not [int64]::TryParse([string]$Value, [Globalization.NumberStyles]::Integer, [Globalization.CultureInfo]::InvariantCulture, [ref]$milliseconds)) { return $null }
    try { $parsed = [DateTimeOffset]::FromUnixTimeMilliseconds($milliseconds) } catch { return $null }
  } else {
    $text = [string]$Value
    if ($text -notmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,7})?(?:Z|[+-]\d{2}:\d{2})$') { return $null }
    if (-not [DateTimeOffset]::TryParse($text, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::None, [ref]$parsed)) { return $null }
  }
  if ($parsed.UtcDateTime -lt [DateTime]::new(2000, 1, 1, 0, 0, 0, [DateTimeKind]::Utc) -or $parsed.UtcDateTime -gt [DateTime]::UtcNow.AddDays(1)) { return $null }
  return $parsed.UtcDateTime.ToString('o')
}

function New-SectionFailure([string]$Section, $ErrorRecord) {
  $exception = Get-SafeProperty $ErrorRecord 'Exception'
  return [pscustomobject]@{
    section = $Section
    available = $false
    errorType = if ($null -eq $exception) { 'Error' } else { Protect-Text $exception.GetType().FullName }
    error = Protect-Text ([string](Get-SafeProperty $exception 'Message' $ErrorRecord))
  }
}

function Write-CollectorSection([string]$Path, [string]$Section, [scriptblock]$Collect) {
  try {
    $value = & $Collect
    Write-JsonFile $Path $value
    return $true
  } catch {
    $failure = New-SectionFailure $Section $_
    try { Write-JsonFile $Path $failure } catch {
      [IO.File]::WriteAllText($Path, "{`"section`":`"$Section`",`"available`":false,`"error`":`"failed to serialize section result`"}`r`n", [Text.UTF8Encoding]::new($false))
    }
    return $false
  }
}

function Protect-StableIdentifier([AllowNull()][string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
  return "<ID:$((Get-ShortHash $Value))>"
}

function Protect-AccountName([AllowNull()][string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
  if ($Value -in @('LocalSystem','NT AUTHORITY\LocalService','NT AUTHORITY\NetworkService')) { return $Value }
  return "<ACCOUNT:$((Get-ShortHash $Value))>"
}

function Protect-Text([AllowNull()][string]$Text) {
  if ($null -eq $Text) { return $null }
  $value = $Text
  if (-not [string]::IsNullOrEmpty($script:UserProfile)) {
    $value = [regex]::Replace($value, [regex]::Escape($script:UserProfile), '<USERPROFILE>', [Text.RegularExpressions.RegexOptions]::IgnoreCase)
  }
  # Quoted paths may contain spaces. The unquoted form is deliberately bounded
  # rather than guessing where arbitrary prose after a path ends.
  $value = [regex]::Replace($value, '(?i)(?<quote>["''])[A-Z]:\\Users\\(?:\\.|(?!\k<quote>).)+\k<quote>', '${quote}<USERPROFILE>${quote}')
  # An unquoted Windows profile path has no universal delimiter. Bound it at
  # structured separators or end-of-line so spaces inside the path are kept.
  $value = [regex]::Replace($value, '(?i)(?<![A-Za-z0-9])[A-Z]:\\Users\\[^\r\n"'',;|]+(?=\s*(?:[\r\n"'',;|]|$))', '<USERPROFILE>')
  if (-not [string]::IsNullOrEmpty($script:UserName)) {
    $escapedUser = [regex]::Escape($script:UserName)
    $value = [regex]::Replace($value, "(?i)(\b(?:user(?:name)?|account)\s*[=:]\s*[`"']?)$escapedUser(?=[`"'\s,;]|$)", '$1<USER>')
    $value = [regex]::Replace($value, "(?i)(\b[A-Za-z0-9_.-]+\\)$escapedUser\b", '$1<USER>')
  }
  $value = [regex]::Replace($value, '\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b', '<REDACTED-ACCOUNT>', 'IgnoreCase')
  $wellKnownSidPattern = '^(?:S-1-0-0|S-1-1-0|S-1-2-[01]|S-1-3-[0-4]|S-1-5-(?:[1-9]|1\d|20|32-\d+)|S-1-16-\d+)$'
  $value = [regex]::Replace($value, '(?i)(?<![A-Za-z0-9-])S-\d+-\d+(?:-\d+)+(?![A-Za-z0-9-])', {
    param($match)
    if ($match.Value -match $wellKnownSidPattern) { return $match.Value }
    return "<SID:$((Get-ShortHash $match.Value))>"
  })

  $credentialNames = 'token|access[_-]?token|refresh[_-]?token|api[_-]?key|apikey|secret|password|passwd|credential|authorization|capability|elevated[_-]?capability|diagnostic[_-]?capability'
  $value = [regex]::Replace($value, "(?i)(?<prefix>[`"']?(?:$credentialNames)[`"']?\s*[:=]\s*)(?<quote>[`"'])(?:\\.|(?!\k<quote>).)*\k<quote>", '${prefix}${quote}<REDACTED>${quote}')
  $value = [regex]::Replace($value, "(?im)(?<prefix>\b(?:$credentialNames)\b\s*[:=]\s*)(?![`"'])(?<secret>[^\r\n,;]+)", '${prefix}<REDACTED>')
  $value = [regex]::Replace($value, '(?im)(?<prefix>\bauthorization\s*:\s*)(?<secret>[^\r\n]+)', '${prefix}<REDACTED>')
  $value = [regex]::Replace($value, '(?i)(-{1,2}(?:token|api-key|secret|password|credential|capability|elevatedcapability|elevated-capability|diagnostic-capability)(?:=|\s+))(?:(?<quote>["'']).*?\k<quote>|[^\s"'']+)', '$1<REDACTED>')
  $value = [regex]::Replace($value, '(?i)(https?://)[^/@\s]+@', '$1<REDACTED>@')
  $value = [regex]::Replace($value, '(?i)([?&](?:token|access_token|refresh_token|api_key|apikey|key|secret|password|authorization|signature|sig)=)[^&#\s"'']+', '$1<REDACTED>')
  $value = [regex]::Replace($value, '(?i)\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]{4,}', '<REDACTED-AUTH>')
  return $value
}

function Protect-Object($Value) {
  if ($null -eq $Value) { return $null }
  if ($Value -is [string]) { return Protect-Text $Value }
  if ($Value -is [Collections.IDictionary]) {
    $result = [ordered]@{}
    foreach ($key in $Value.Keys) {
      if ([string]$key -match '(?i)^(?:token|access[_-]?token|refresh[_-]?token|secret|password|passwd|credential|authorization|capability|elevatedCapability|diagnosticCapability|api[_-]?key|apikey)$') { $result[[string]$key] = '<REDACTED>' }
      elseif ([string]$key -match '(?i)^installationId$') { $result[[string]$key] = Protect-StableIdentifier ([string]$Value[$key]) }
      else { $result[[string]$key] = Protect-Object $Value[$key] }
    }
    return [pscustomobject]$result
  }
  if (($Value -is [Collections.IEnumerable]) -and -not ($Value -is [string])) {
    $items = @()
    foreach ($item in $Value) { $items += ,(Protect-Object $item) }
    return ,$items
  }
  if ($Value -is [psobject] -and @($Value.PSObject.Properties).Count -gt 0) {
    $result = [ordered]@{}
    foreach ($property in $Value.PSObject.Properties) {
      if ($property.Name -match '(?i)^(?:token|access[_-]?token|refresh[_-]?token|secret|password|passwd|credential|authorization|capability|elevatedCapability|diagnosticCapability|api[_-]?key|apikey)$') { $result[$property.Name] = '<REDACTED>' }
      elseif ($property.Name -match '(?i)^installationId$') { $result[$property.Name] = Protect-StableIdentifier ([string]$property.Value) }
      else { $result[$property.Name] = Protect-Object $property.Value }
    }
    return [pscustomobject]$result
  }
  return $Value
}

function ConvertTo-ProtectedJson($Value) {
  $protected = Protect-Object $Value
  return (ConvertTo-Json -InputObject $protected -Depth 12)
}

function Write-JsonFile([string]$Path, $Value) {
  $json = ConvertTo-ProtectedJson $Value
  [IO.File]::WriteAllText($Path, "$json`r`n", [Text.UTF8Encoding]::new($false))
}

function Set-CollectorStagingAcl([string]$Path) {
  $security = [Security.AccessControl.DirectorySecurity]::new()
  $security.SetOwner([Security.Principal.SecurityIdentifier]::new($script:CurrentSid))
  $security.SetAccessRuleProtection($true, $false)
  foreach ($sid in @($script:CurrentSid, 'S-1-5-18', 'S-1-5-32-544')) {
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    [void]$security.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $Path -AclObject $security
}

function Set-CollectorOutputDirectoryAcl([string]$Path) {
  $security = [Security.AccessControl.DirectorySecurity]::new()
  $currentSid = [Security.Principal.SecurityIdentifier]::new($script:CurrentSid)
  $security.SetOwner($currentSid)
  $security.SetAccessRuleProtection($true, $false)
  foreach ($sid in @($script:CurrentSid, 'S-1-5-18')) {
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    [void]$security.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $Path -AclObject $security -ErrorAction Stop
}

function Set-CollectorArchiveAcl([string]$Path) {
  $security = [Security.AccessControl.FileSecurity]::new()
  $currentSid = [Security.Principal.SecurityIdentifier]::new($script:CurrentSid)
  $security.SetOwner($currentSid)
  $security.SetAccessRuleProtection($true, $false)
  foreach ($sid in @($script:CurrentSid, 'S-1-5-18')) {
    $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow)
    [void]$security.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $Path -AclObject $security -ErrorAction Stop
  $verified = Get-Acl -LiteralPath $Path -ErrorAction Stop
  if (-not [bool](Get-SafeProperty $verified 'AreAccessRulesProtected' $false)) { throw 'Diagnostic ZIP ACL inheritance is not disabled.' }
  foreach ($rule in @(Get-SafeProperty $verified 'Access' @())) {
    $identityReference = Get-SafeProperty $rule 'IdentityReference'
    try { $identity = [string](Get-SafeProperty ($identityReference.Translate([Security.Principal.SecurityIdentifier])) 'Value') } catch { throw 'Diagnostic ZIP ACL identity could not be validated.' }
    if ($identity -notin @($script:CurrentSid, 'S-1-5-18')) { throw 'Diagnostic ZIP ACL contains an unexpected identity.' }
  }
}

function Get-VerifiedRecoveryOutputRoot {
  $localAppData = Get-TrustedKnownFolderPath 'f1b32785-6fba-4fcf-9d55-7b8e7f157091'
  $applicationRoot = Join-Path $localAppData 'Talking Quill'
  $diagnosticsRoot = Join-Path $applicationRoot 'Diagnostics'
  if (-not (Test-NoReparsePointInPath $applicationRoot)) { throw 'The per-user recovery path contains a reparse point.' }
  New-Item -ItemType Directory -Path $diagnosticsRoot -Force -ErrorAction Stop | Out-Null
  if (-not (Test-NoReparsePointInPath $diagnosticsRoot)) { throw 'The per-user diagnostics path contains a reparse point.' }
  Set-CollectorOutputDirectoryAcl $diagnosticsRoot
  return [IO.Path]::GetFullPath($diagnosticsRoot).TrimEnd('\')
}

function Get-VerifiedOutputRoot([AllowNull()][string]$RequestedPath) {
  $candidate = if ([string]::IsNullOrWhiteSpace($RequestedPath)) {
    Get-TrustedKnownFolderPath '374de290-123f-4565-9164-39c4925e467b'
  } else {
    $RequestedPath
  }
  $fullPath = [IO.Path]::GetFullPath($candidate).TrimEnd('\')
  if (-not [IO.Path]::IsPathRooted($fullPath) -or $fullPath -match '^(?:\\\\|\\\\[?.]\\)') {
    throw 'The diagnostic output directory must be a local absolute path.'
  }
  if (-not (Test-Path -LiteralPath $fullPath)) { New-Item -ItemType Directory -Path $fullPath -Force -ErrorAction Stop | Out-Null }
  if (-not (Test-Path -LiteralPath $fullPath -PathType Container -ErrorAction Stop) -or -not (Test-NoReparsePointInPath $fullPath)) {
    throw 'The diagnostic output directory is not a trusted local directory.'
  }
  return $fullPath
}

function Initialize-SecureStaging([string]$Path) {
  try {
    New-Item -ItemType Directory -Path $Path -Force -ErrorAction Stop | Out-Null
    Set-CollectorStagingAcl $Path
    return $true
  } catch {
    Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
    throw
  }
}

function Get-TrustedWindowsPowerShellPath {
  $candidate = Join-Path $PSHOME 'powershell.exe'
  $fullCandidate = [IO.Path]::GetFullPath($candidate)
  $fullHome = [IO.Path]::GetFullPath($PSHOME).TrimEnd('\')
  if (-not [IO.Path]::IsPathRooted($fullCandidate) -or $fullCandidate -match '^(?:\\\\|\\\\[?.]\\)' -or [string]::Compare((Split-Path -Parent $fullCandidate).TrimEnd('\'), $fullHome, $true, [Globalization.CultureInfo]::InvariantCulture) -ne 0 -or (Split-Path -Leaf $fullCandidate) -ine 'powershell.exe') {
    throw 'The Windows PowerShell executable path is not trusted.'
  }
  if (-not (Test-Path -LiteralPath $fullCandidate -PathType Leaf -ErrorAction Stop)) { throw 'The trusted Windows PowerShell executable is unavailable.' }
  $item = Get-Item -LiteralPath $fullCandidate -Force -ErrorAction Stop
  if (((Get-SafeProperty $item 'Attributes' 0) -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or -not (Test-NoReparsePointInPath $fullCandidate)) { throw 'The Windows PowerShell executable path contains a reparse point.' }
  return $fullCandidate
}

function New-CollectorPipeSecurity {
  $security = [IO.Pipes.PipeSecurity]::new()
  $security.SetAccessRuleProtection($true, $false)
  $currentSid = [Security.Principal.SecurityIdentifier]::new($script:CurrentSid)
  $security.SetOwner($currentSid)
  $security.AddAccessRule([IO.Pipes.PipeAccessRule]::new($currentSid, [IO.Pipes.PipeAccessRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow))
  return $security
}

function Get-RandomCapability {
  $bytes = New-Object byte[] 32
  $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $generator.GetBytes($bytes) } finally { $generator.Dispose() }
  return ([BitConverter]::ToString($bytes)).Replace('-', '').ToLowerInvariant()
}

function Test-CapabilityEqual([AllowNull()][string]$Expected, [AllowNull()][string]$Actual) {
  if ($null -eq $Expected -or $null -eq $Actual -or $Expected.Length -ne 64 -or $Actual.Length -ne 64) { return $false }
  $difference = 0
  for ($index = 0; $index -lt 64; $index++) { $difference = $difference -bor ([int]$Expected[$index] -bxor [int]$Actual[$index]) }
  return $difference -eq 0
}

function Initialize-WindowsPathResolver {
  if ('TqWindowsPathResolver' -as [type]) { return }
  Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class TqWindowsPathResolver {
  [DllImport("shell32.dll", CharSet=CharSet.Unicode, PreserveSig=true)]
  static extern int SHGetKnownFolderPath(ref Guid folderId, uint flags, IntPtr token, out IntPtr path);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern uint GetSystemDirectory(StringBuilder buffer, uint size);
  public static string KnownFolder(string id) {
    Guid folderId = new Guid(id); IntPtr path = IntPtr.Zero;
    int result = SHGetKnownFolderPath(ref folderId, 0, IntPtr.Zero, out path);
    if (result != 0) throw new Win32Exception(result);
    try { return Marshal.PtrToStringUni(path); } finally { Marshal.FreeCoTaskMem(path); }
  }
  public static string SystemDirectory() {
    StringBuilder buffer = new StringBuilder(32768);
    uint result = GetSystemDirectory(buffer, (uint)buffer.Capacity);
    if (result == 0 || result >= buffer.Capacity) throw new Win32Exception(Marshal.GetLastWin32Error());
    return buffer.ToString();
  }
}
'@
}

function Get-TrustedKnownFolderPath([string]$FolderId) {
  Initialize-WindowsPathResolver
  $path = [TqWindowsPathResolver]::KnownFolder($FolderId)
  if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.Path]::IsPathRooted($path) -or $path -match '^(?:\\\\|\\\\[?.]\\)') { throw 'Windows returned an untrusted known-folder path.' }
  $fullPath = [IO.Path]::GetFullPath($path).TrimEnd('\')
  if (-not (Test-NoReparsePointInPath $fullPath)) { throw 'A trusted known-folder path contains a reparse point.' }
  return $fullPath
}

function Get-TrustedSystemDirectory {
  Initialize-WindowsPathResolver
  $path = [TqWindowsPathResolver]::SystemDirectory()
  if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.Path]::IsPathRooted($path) -or $path -match '^(?:\\\\|\\\\[?.]\\)') { throw 'Windows returned an untrusted system directory.' }
  $fullPath = [IO.Path]::GetFullPath($path).TrimEnd('\')
  if (-not (Test-NoReparsePointInPath $fullPath)) { throw 'The Windows system directory contains a reparse point.' }
  return $fullPath
}

function Get-ExpectedInstallRoots {
  $roots = @()
  foreach ($folderId in @('905e63b6-c1bf-494e-b29c-65b732d3d21a','6d809377-6af0-444b-8957-a3773f02200e','7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e')) {
    try {
      $programFilesPath = Get-TrustedKnownFolderPath $folderId
      $candidate = [IO.Path]::GetFullPath((Join-Path $programFilesPath 'Talking Quill')).TrimEnd('\')
      if (Test-NoReparsePointInPath $candidate) { $roots += $candidate }
    } catch { continue }
  }
  $roots = @($roots | Select-Object -Unique)
  if ($roots.Count -eq 0) { throw 'Windows did not return a trusted Program Files location.' }
  return $roots
}

function Test-NoReparsePointInPath([string]$Path) {
  $fullPath = [IO.Path]::GetFullPath($Path)
  $root = [IO.Path]::GetPathRoot($fullPath)
  $current = $root
  foreach ($segment in @($fullPath.Substring($root.Length).Split('\') | Where-Object { $_ })) {
    $current = Join-Path $current $segment
    if (-not (Test-Path -LiteralPath $current -ErrorAction Stop)) { break }
    $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
    if (((Get-SafeProperty $item 'Attributes' 0) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { return $false }
  }
  return $true
}

function Resolve-TrustedInstallRoot([AllowNull()][string]$Path) {
  if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
  $candidate = $Path.Trim().Trim('"')
  if ($candidate -match '^(?:\\\\|\\\\[?.]\\|\\\?\?\\)' -or -not [IO.Path]::IsPathRooted($candidate)) { return $null }
  try { $fullPath = [IO.Path]::GetFullPath($candidate).TrimEnd('\') } catch { return $null }
  $trusted = @((Get-ExpectedInstallRoots) | Where-Object { [string]::Equals($_, $fullPath, [StringComparison]::OrdinalIgnoreCase) })
  if ($trusted.Count -ne 1) { return $null }
  try { if (-not (Test-NoReparsePointInPath $fullPath)) { return $null } } catch { return $null }
  return $trusted[0]
}

function Resolve-TrustedInstalledPath([string]$Root, [string]$RelativePath) {
  $trustedRoot = Resolve-TrustedInstallRoot $Root
  if ($null -eq $trustedRoot -or [IO.Path]::IsPathRooted($RelativePath) -or $RelativePath -match '(^|[\\/])\.\.([\\/]|$)') { return $null }
  try { $candidate = [IO.Path]::GetFullPath((Join-Path $trustedRoot $RelativePath)) } catch { return $null }
  $prefix = "$trustedRoot\"
  if (-not $candidate.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { return $null }
  try { if (-not (Test-NoReparsePointInPath $candidate)) { return $null } } catch { return $null }
  return $candidate
}

function Get-SafeCommandLine([AllowNull()][string]$CommandLine, [string]$ProcessName) {
  if ([string]::IsNullOrWhiteSpace($CommandLine)) { return $null }
  $tokens = @([regex]::Matches($CommandLine, '"[^"]*"|\S+') | ForEach-Object { $_.Value.Trim('"') })
  $safe = @($ProcessName)
  $allowedFlags = @('type','lang','log-level','ozone-platform','device-scale-factor','user-data-dir','resources-path','no-sandbox','disable-gpu','disable-features','enable-features','field-trial-handle','variations-seed-version','prefetch','talking-quill-installed-readiness-pipe')
  foreach ($token in @($tokens | Select-Object -Skip 1)) {
    if ($token -match '^--(?<name>[a-z0-9-]{1,64})(?:=.*)?$' -and $allowedFlags -contains $Matches.name) { $safe += "--$($Matches.name)=<REDACTED>" }
    elseif ($token -match '^--') { $safe += '<REDACTED-FLAG>' }
    else { $safe += '<REDACTED-ARG>' }
  }
  return ($safe -join ' ')
}

function Get-FileMetadata([string]$Path, [switch]$Hash) {
  if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{ path = Protect-Text $Path; exists = $false } }
  try {
    $item = Get-Item -LiteralPath $Path -Force
    $isContainer = [bool](Get-SafeProperty $item 'PSIsContainer' $false)
    $version = if (-not $isContainer) { Get-SafeProperty $item 'VersionInfo' } else { $null }
    $record = [ordered]@{
      path = Protect-Text ([string](Get-SafeProperty $item 'FullName' $Path))
      exists = $true
      type = if ($isContainer) { 'directory' } else { 'file' }
      length = if ($isContainer) { $null } else { Get-SafeProperty $item 'Length' }
      createdUtc = ConvertTo-SafeUtcText (Get-SafeProperty $item 'CreationTimeUtc')
      modifiedUtc = ConvertTo-SafeUtcText (Get-SafeProperty $item 'LastWriteTimeUtc')
      attributes = [string](Get-SafeProperty $item 'Attributes')
    }
    if ($null -ne $version) {
      $record.fileVersion = Protect-Text ([string](Get-SafeProperty $version 'FileVersion'))
      $record.productVersion = Protect-Text ([string](Get-SafeProperty $version 'ProductVersion'))
      $record.companyName = Protect-Text ([string](Get-SafeProperty $version 'CompanyName'))
      $record.productName = Protect-Text ([string](Get-SafeProperty $version 'ProductName'))
    }
    if ($Hash -and -not $isContainer) { $record.sha256 = [string](Get-SafeProperty (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop) 'Hash').ToLowerInvariant() }
    return [pscustomobject]$record
  } catch { return [pscustomobject]@{ path = Protect-Text $Path; exists = $true; error = Protect-Text $_.Exception.Message } }
}

function Get-InstallRecords {
  $records = @()
  $script:InstallQueryErrors = @()
  $uninstallPath = 'Software\Microsoft\Windows\CurrentVersion\Uninstall'
  $queries = @(
    [pscustomobject]@{ hive = 'HKLM'; registryHive = [Microsoft.Win32.RegistryHive]::LocalMachine; view = [Microsoft.Win32.RegistryView]::Registry64; viewName = '64-bit' }
    [pscustomobject]@{ hive = 'HKLM'; registryHive = [Microsoft.Win32.RegistryHive]::LocalMachine; view = [Microsoft.Win32.RegistryView]::Registry32; viewName = '32-bit' }
    [pscustomobject]@{ hive = 'HKCU'; registryHive = [Microsoft.Win32.RegistryHive]::CurrentUser; view = [Microsoft.Win32.RegistryView]::Registry64; viewName = '64-bit' }
    [pscustomobject]@{ hive = 'HKCU'; registryHive = [Microsoft.Win32.RegistryHive]::CurrentUser; view = [Microsoft.Win32.RegistryView]::Registry32; viewName = '32-bit' }
  )
  foreach ($query in $queries) {
    $baseKey = $null
    $rootKey = $null
    try {
      $baseKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey($query.registryHive, $query.view)
      $rootKey = $baseKey.OpenSubKey($uninstallPath, $false)
      if ($null -eq $rootKey) { continue }
      foreach ($keyName in @($rootKey.GetSubKeyNames())) {
        $item = $null
        try {
          $item = $rootKey.OpenSubKey($keyName, $false)
          if ($null -eq $item) { continue }
          $displayName = [string]$item.GetValue('DisplayName', $null)
          if ($displayName -notmatch '^Talking Quill') { continue }
          $reportedInstallLocation = [string]$item.GetValue('InstallLocation', $null)
          $installLocationRaw = Resolve-TrustedInstallRoot $reportedInstallLocation
          $installLocationStatus = if ([string]::IsNullOrWhiteSpace($reportedInstallLocation)) { 'not-reported' } elseif ($null -ne $installLocationRaw) { 'trusted-local-root' } else { 'rejected-untrusted-path' }
          $records += [pscustomobject]@{
            hive = $query.hive
            registryView = $query.viewName
            keyNameHash = Get-ShortHash $keyName
            displayName = $displayName
            displayVersion = $item.GetValue('DisplayVersion', $null)
            publisher = $item.GetValue('Publisher', $null)
            installLocation = if ($null -ne $installLocationRaw) { Protect-Text $installLocationRaw } else { $null }
            installLocationStatus = $installLocationStatus
            installLocationRaw = $installLocationRaw
            displayIcon = if ($null -eq $item.GetValue('DisplayIcon', $null)) { $null } else { '<PRESENT-NOT-COLLECTED>' }
            installDate = $item.GetValue('InstallDate', $null)
            estimatedSizeKiB = $item.GetValue('EstimatedSize', $null)
            noModify = $item.GetValue('NoModify', $null)
            noRepair = $item.GetValue('NoRepair', $null)
          }
        } catch {
          $script:InstallQueryErrors += [pscustomobject]@{ hive = $query.hive; registryView = $query.viewName; reason = if (Test-AccessDenied $_) { 'key-access-denied' } else { 'key-query-failed' } }
        } finally { if ($null -ne $item) { $item.Dispose() } }
      }
    } catch {
      $script:InstallQueryErrors += [pscustomobject]@{ hive = $query.hive; registryView = $query.viewName; reason = if (Test-AccessDenied $_) { 'access-denied' } else { 'query-failed' } }
    } finally {
      if ($null -ne $rootKey) { $rootKey.Dispose() }
      if ($null -ne $baseKey) { $baseKey.Dispose() }
    }
  }
  $deduplicated = [ordered]@{}
  foreach ($record in $records) {
    $fingerprint = Get-ShortHash ((@(
      Get-SafeProperty $record 'hive'
      Get-SafeProperty $record 'keyNameHash'
      Get-SafeProperty $record 'displayName'
      Get-SafeProperty $record 'displayVersion'
      Get-SafeProperty $record 'installLocationRaw'
    ) | ForEach-Object { [string]$_ }) -join "`n")
    $viewName = [string](Get-SafeProperty $record 'registryView')
    if ($deduplicated.Contains($fingerprint)) {
      $existing = $deduplicated[$fingerprint]
      $views = @(@(Get-SafeProperty $existing 'registryViews' @()) + $viewName | Where-Object { $_ } | Select-Object -Unique)
      $existing | Add-Member -NotePropertyName registryViews -NotePropertyValue $views -Force
    } else {
      $record | Add-Member -NotePropertyName registryViews -NotePropertyValue @($viewName) -Force
      $deduplicated[$fingerprint] = $record
    }
  }
  $script:InstallQueryErrors = @($script:InstallQueryErrors | Sort-Object hive,registryView,reason -Unique)
  return @($deduplicated.Values)
}

function Get-InstalledFiles($InstallRecords) {
  $roots = @(Get-ExpectedInstallRoots)
  foreach ($record in @($InstallRecords)) {
    $location = Resolve-TrustedInstallRoot ([string](Get-SafeProperty $record 'installLocationRaw'))
    if ($null -ne $location) { $roots += $location }
  }
  $roots = @($roots | Select-Object -Unique)
  $relative = @(
    'Talking Quill.exe',
    'resources\helper\talking-quill-helper.exe',
    'resources\helper\talking-quill-keyboard-owner.exe',
    'resources\keyboard-owner-release-v1.json',
    'resources\app.asar'
  )
  $output = @()
  foreach ($root in $roots) {
    foreach ($name in $relative) {
      $trustedPath = Resolve-TrustedInstalledPath $root $name
      if ($null -eq $trustedPath) {
        $output += [pscustomobject]@{ installRoot = Protect-Text $root; relativePath = $name; exists = $false; error = 'rejected-untrusted-or-reparse-path' }
      } else { $output += Get-FileMetadata $trustedPath -Hash }
    }
  }
  return $output
}

function Initialize-TokenInspector {
  if ('TqTokenInspector' -as [type]) { return }
  Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class TqTokenInspector {
  [StructLayout(LayoutKind.Sequential)] struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetTokenInformation(IntPtr token, int cls, IntPtr info, int length, out int required);
  [DllImport("advapi32.dll", SetLastError=true)] static extern IntPtr GetSidSubAuthority(IntPtr sid, uint index);
  [DllImport("advapi32.dll", SetLastError=true)] static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  public static string Integrity(int pid) {
    IntPtr process=IntPtr.Zero, token=IntPtr.Zero, buffer=IntPtr.Zero;
    try {
      process=OpenProcess(0x1000, false, pid); if(process==IntPtr.Zero) return "unavailable:"+Marshal.GetLastWin32Error();
      if(!OpenProcessToken(process, 0x0008, out token)) return "unavailable:"+Marshal.GetLastWin32Error();
      int needed=0; GetTokenInformation(token, 25, IntPtr.Zero, 0, out needed);
      buffer=Marshal.AllocHGlobal(needed); if(!GetTokenInformation(token, 25, buffer, needed, out needed)) return "unavailable:"+Marshal.GetLastWin32Error();
      var item=(SID_AND_ATTRIBUTES)Marshal.PtrToStructure(buffer, typeof(SID_AND_ATTRIBUTES));
      byte count=Marshal.ReadByte(GetSidSubAuthorityCount(item.Sid)); uint rid=(uint)Marshal.ReadInt32(GetSidSubAuthority(item.Sid, (uint)(count-1)));
      if(rid < 0x1000) return "untrusted"; if(rid < 0x2000) return "low"; if(rid < 0x3000) return "medium"; if(rid < 0x4000) return "high"; if(rid < 0x5000) return "system"; return "protected";
    } finally { if(buffer!=IntPtr.Zero)Marshal.FreeHGlobal(buffer); if(token!=IntPtr.Zero)CloseHandle(token); if(process!=IntPtr.Zero)CloseHandle(process); }
  }
}
'@
}

function Get-TalkingQuillProcesses {
  Initialize-TokenInspector
  $all = @(Get-CimInstance Win32_Process -ErrorAction Stop)
  $byPid = @{}
  foreach ($process in $all) {
    $processId = ConvertTo-SafeInt (Get-SafeProperty $process 'ProcessId') -1
    if ($processId -ge 0) { $byPid[$processId] = $process }
  }
  $result = @()
  foreach ($process in $all) {
    $name = [string](Get-SafeProperty $process 'Name')
    if ($name -notmatch $script:RelevantProcessPattern) { continue }
    $processId = ConvertTo-SafeInt (Get-SafeProperty $process 'ProcessId') -1
    $parentId = ConvertTo-SafeInt (Get-SafeProperty $process 'ParentProcessId') -1
    $parent = if ($byPid.ContainsKey($parentId)) { $byPid[$parentId] } else { $null }
    $sessionId = try { Get-SafeProperty (Get-Process -Id $processId -ErrorAction Stop) 'SessionId' } catch { $null }
    $result += [pscustomobject]@{
      name = $name
      pid = $processId
      parentPid = $parentId
      parentName = Get-SafeProperty $parent 'Name'
      sessionId = $sessionId
      integrity = if ($processId -ge 0) { [TqTokenInspector]::Integrity($processId) } else { 'unavailable:invalid-pid' }
      executablePath = Protect-Text ([string](Get-SafeProperty $process 'ExecutablePath'))
      commandLine = Get-SafeCommandLine ([string](Get-SafeProperty $process 'CommandLine')) $name
      creationUtc = ConvertTo-SafeUtcText (Get-SafeProperty $process 'CreationDate')
    }
  }
  return $result
}

function Get-ManifestSummary([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{ path = Protect-Text $Path; exists = $false } }
  try {
    $source = Get-Content -LiteralPath $Path -Raw
    $manifest = $source | ConvertFrom-Json
    $summary = [ordered]@{ path = Protect-Text $Path; exists = $true; length = ([Text.Encoding]::UTF8.GetByteCount($source)); sha256 = (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
    foreach ($name in @('schemaVersion','kind','platform','architecture','ownerMode','version','releaseVersion','releaseBuildDigest','packageLayoutDigest','buildId','installationId')) {
      $property = Get-SafeProperty $manifest $name
      if ($null -ne $property) { $summary[$name] = Protect-Text ([string]$property) }
    }
    $roles = Get-SafeProperty $manifest 'roles'
    if ($null -ne $roles) {
      $summary.roles = @($roles | ForEach-Object { [pscustomobject]@{ role = Get-SafeProperty $_ 'role'; path = Protect-Text ([string](Get-SafeProperty $_ 'path')); sha256 = Get-SafeProperty $_ 'sha256' } })
    }
    $images = Get-SafeProperty $manifest 'images'
    if ($null -ne $images) {
      $summary.images = @($images | ForEach-Object { [pscustomobject]@{ role = Get-SafeProperty $_ 'role'; path = Protect-Text ([string](Get-SafeProperty $_ 'path')); sha256 = Get-SafeProperty $_ 'sha256' } })
    }
    return [pscustomobject]$summary
  } catch { return [pscustomobject]@{ path = Protect-Text $Path; exists = $true; error = Protect-Text $_.Exception.Message } }
}

function Get-AclSummary([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{ path = Protect-Text $Path; exists = $false } }
  try {
    $item = Get-Item -LiteralPath $Path -Force
    $acl = Get-Acl -LiteralPath $Path
    return [pscustomobject]@{
      path = Protect-Text $Path
      exists = $true
      reparsePoint = (((Get-SafeProperty $item 'Attributes' 0) -band [IO.FileAttributes]::ReparsePoint) -ne 0)
      ownerSid = Protect-Text ([string](Get-SafeProperty ($acl.GetOwner([Security.Principal.SecurityIdentifier])) 'Value'))
      sddl = Protect-Text $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
    }
  } catch { return [pscustomobject]@{ path = Protect-Text $Path; exists = $true; error = Protect-Text $_.Exception.Message } }
}

function Get-NumericDiagnosticTree($Value) {
  if ($null -eq $Value -or $Value -is [bool] -or $Value -is [int] -or $Value -is [long] -or $Value -is [double] -or $Value -is [decimal]) { return $Value }
  if ($Value -isnot [psobject]) { return $null }
  $result = [ordered]@{}
  foreach ($property in @($Value.PSObject.Properties)) {
    if ($property.Name -notmatch '^[A-Za-z][A-Za-z0-9]{0,63}$') { continue }
    $safe = Get-NumericDiagnosticTree (Get-SafeProperty $Value $property.Name)
    if ($null -ne $safe) { $result[$property.Name] = $safe }
  }
  return [pscustomobject]$result
}

function Get-SafeDiagnosticMetadata($Metadata) {
  if ($null -eq $Metadata) { return [pscustomobject]@{} }
  $safe = [ordered]@{}
  foreach ($name in @('component','outcome','reason','code')) {
    $value = [string](Get-SafeProperty $Metadata $name)
    if ($value -match '^[A-Za-z0-9_.-]{1,80}$') { $safe[$name] = $value }
  }
  $diagnosticId = [string](Get-SafeProperty $Metadata 'diagnosticId')
  if ($diagnosticId -match '^[0-9a-f-]{36}$') { $safe.diagnosticId = $diagnosticId }
  $o = Get-SafeProperty $Metadata 'observability'
  if ($null -ne $o) {
    $keyboardOwner = Get-SafeProperty $o 'keyboardOwner'
    $model = [string](Get-SafeProperty $keyboardOwner 'model')
    $state = [string](Get-SafeProperty $keyboardOwner 'state')
    $buildId = [string](Get-SafeProperty $keyboardOwner 'buildId')
    $safe.observability = [pscustomobject]@{
      keyboardOwner = [pscustomobject]@{
        model = if ($model -eq 'out_of_process') { 'out_of_process' } else { 'unknown' }
        protocolVersion = ConvertTo-SafeInt (Get-SafeProperty $keyboardOwner 'protocolVersion')
        state = if ($state -match '^[a-z_]{1,32}$') { $state } else { 'unknown' }
        buildId = if ($buildId -match '^[A-Za-z0-9_.-]{0,128}$') { $buildId } else { '<REDACTED>' }
        leaseEpoch = Get-SafeProperty $keyboardOwner 'leaseEpoch'
        authenticated = [bool](Get-SafeProperty $keyboardOwner 'authenticated' $false)
      }
      owner = Get-NumericDiagnosticTree (Get-SafeProperty $o 'owner')
      registeredInput = Get-NumericDiagnosticTree (Get-SafeProperty $o 'registeredInput')
      keyboardCapture = Get-NumericDiagnosticTree (Get-SafeProperty $o 'keyboardCapture')
      transactions = Get-NumericDiagnosticTree (Get-SafeProperty $o 'transactions')
      replay = Get-NumericDiagnosticTree (Get-SafeProperty $o 'replay')
      dummy = Get-NumericDiagnosticTree (Get-SafeProperty $o 'dummy')
      paste = Get-NumericDiagnosticTree (Get-SafeProperty $o 'paste')
    }
  }
  return [pscustomobject]$safe
}

function Get-DiagnosticTail([string]$LogsPath) {
  $entries = @()
  $invalidTimestampCount = 0
  if (-not (Test-Path -LiteralPath $LogsPath -ErrorAction SilentlyContinue)) { $files = @() }
  else { $files = @(Get-ChildItem -LiteralPath $LogsPath -Filter 'diagnostic.jsonl*' -File -ErrorAction Stop | Where-Object { (Get-SafeProperty $_ 'Name') -match '^diagnostic\.jsonl(?:\.[1-3])?$' } | Sort-Object LastWriteTimeUtc) }
  foreach ($file in $files) {
    $filePath = [string](Get-SafeProperty $file 'FullName')
    foreach ($line in @(Get-Content -LiteralPath $filePath -Tail 256 -ErrorAction Stop)) {
      try {
        $entry = $line | ConvertFrom-Json
        $eventName = [string](Get-SafeProperty $entry 'event')
        if ($script:AllowedDiagnosticEvents -notcontains $eventName) { continue }
        $timestamp = ConvertTo-ValidatedDiagnosticTimestamp (Get-SafeProperty $entry 'timestamp')
        if ($null -eq $timestamp) { $invalidTimestampCount++; continue }
        $entries += [pscustomobject]@{ source = Get-SafeProperty $file 'Name'; timestamp = $timestamp; event = $eventName; metadata = Get-SafeDiagnosticMetadata (Get-SafeProperty $entry 'metadata') }
      } catch { }
    }
  }
  $entries = @($entries | Sort-Object { [DateTimeOffset](Get-SafeProperty $_ 'timestamp') })
  if ($entries.Count -gt 512) { $entries = @($entries | Select-Object -Last 512) }
  return [pscustomobject]@{
    logsPath = Protect-Text $LogsPath
    files = @($files | ForEach-Object { Get-FileMetadata ([string](Get-SafeProperty $_ 'FullName')) })
    invalidTimestampCount = $invalidTimestampCount
    acceptedEntryCount = $entries.Count
    entries = $entries
    stdout = 'not persisted by the packaged application'
    stderr = 'not persisted; the app consumes only bounded, schema-checked native terminal observability'
    diagnosticRing = if ($entries.Count -eq 0) { 'empty or diagnostic logging disabled' } else { 'bounded diagnostic.jsonl tail' }
  }
}

function Monitor-TalkingQuillExits([int]$Seconds) {
  $sourceStart = "TqDiagStart-$PID"
  $sourceStop = "TqDiagStop-$PID"
  $events = @()
  $known = @{}
  try {
    Register-WmiEvent -Class Win32_ProcessStartTrace -SourceIdentifier $sourceStart | Out-Null
    Register-WmiEvent -Class Win32_ProcessStopTrace -SourceIdentifier $sourceStop | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
      foreach ($event in @(Get-Event -SourceIdentifier $sourceStart -ErrorAction SilentlyContinue)) {
        $p = Get-SafeProperty (Get-SafeProperty $event 'SourceEventArgs') 'NewEvent'
        $name = [string](Get-SafeProperty $p 'ProcessName')
        $eventProcessId = ConvertTo-SafeInt (Get-SafeProperty $p 'ProcessID') -1
        if ($name -match $script:RelevantProcessPattern) {
          if ($eventProcessId -ge 0) { $known[$eventProcessId] = $true }
          $events += [pscustomobject]@{ type = 'start'; observedUtc = ConvertTo-SafeUtcText (Get-SafeProperty $event 'TimeGenerated'); name = $name; pid = $eventProcessId; parentPid = ConvertTo-SafeInt (Get-SafeProperty $p 'ParentProcessID') -1; sessionId = ConvertTo-SafeInt (Get-SafeProperty $p 'SessionID') -1 }
        }
        $eventIdentifier = Get-SafeProperty $event 'EventIdentifier'
        if ($null -ne $eventIdentifier) { Remove-Event -EventIdentifier $eventIdentifier -ErrorAction SilentlyContinue }
      }
      foreach ($event in @(Get-Event -SourceIdentifier $sourceStop -ErrorAction SilentlyContinue)) {
        $p = Get-SafeProperty (Get-SafeProperty $event 'SourceEventArgs') 'NewEvent'
        $name = [string](Get-SafeProperty $p 'ProcessName')
        $eventProcessId = ConvertTo-SafeInt (Get-SafeProperty $p 'ProcessID') -1
        if ($name -match $script:RelevantProcessPattern -or ($eventProcessId -ge 0 -and $known.ContainsKey($eventProcessId))) {
          $events += [pscustomobject]@{ type = 'exit'; observedUtc = ConvertTo-SafeUtcText (Get-SafeProperty $event 'TimeGenerated'); name = $name; pid = $eventProcessId; parentPid = ConvertTo-SafeInt (Get-SafeProperty $p 'ParentProcessID') -1; sessionId = ConvertTo-SafeInt (Get-SafeProperty $p 'SessionID') -1; exitCode = ConvertTo-SafeInt (Get-SafeProperty $p 'ExitStatus') }
        }
        $eventIdentifier = Get-SafeProperty $event 'EventIdentifier'
        if ($null -ne $eventIdentifier) { Remove-Event -EventIdentifier $eventIdentifier -ErrorAction SilentlyContinue }
      }
      Start-Sleep -Milliseconds 200
    }
    return [pscustomobject]@{ durationSeconds = $Seconds; mechanism = 'Win32_ProcessStartTrace/StopTrace'; relevantEvents = $events; note = if ($events.Count -eq 0) { 'No Talking Quill child start/exit event was observed during the bounded window.' } else { 'Only exact Talking Quill executable names were retained.' } }
  } catch { return [pscustomobject]@{ durationSeconds = $Seconds; mechanism = 'unavailable'; relevantEvents = $events; error = Protect-Text $_.Exception.Message } }
  finally {
    Unregister-Event -SourceIdentifier $sourceStart -ErrorAction SilentlyContinue
    Unregister-Event -SourceIdentifier $sourceStop -ErrorAction SilentlyContinue
    Remove-Event -SourceIdentifier $sourceStart -ErrorAction SilentlyContinue
    Remove-Event -SourceIdentifier $sourceStop -ErrorAction SilentlyContinue
  }
}

function Test-AccessDenied($ErrorRecord) {
  $exception = Get-SafeProperty $ErrorRecord 'Exception'
  if ($null -eq $exception) { return $false }
  return $exception -is [UnauthorizedAccessException] -or (Get-SafeProperty $exception 'HResult') -eq -2147024891 -or [string](Get-SafeProperty $exception 'Message') -match '(?i)access is denied|unauthorized'
}

function Invoke-ScQuery([string]$Verb, [string]$ServiceName) {
  try {
    $scPath = [IO.Path]::GetFullPath((Join-Path (Get-TrustedSystemDirectory) 'sc.exe'))
    if (-not (Test-Path -LiteralPath $scPath -PathType Leaf -ErrorAction Stop) -or -not (Test-NoReparsePointInPath $scPath)) { throw 'sc.exe is unavailable or untrusted.' }
  } catch { return [pscustomobject]@{ status = 'unavailable'; exitCode = $null; text = '' } }
  try {
    $output = @(& $scPath $Verb $ServiceName 2>&1)
    $exitCode = $LASTEXITCODE
    $text = ($output | ForEach-Object { [string]$_ }) -join "`n"
    $status = if ($exitCode -eq 0) { 'ok' } elseif ($exitCode -eq 5 -or $text -match '(?i)access is denied') { 'access-denied' } else { 'unavailable' }
    return [pscustomobject]@{ status = $status; exitCode = $exitCode; text = $text }
  } catch {
    return [pscustomobject]@{ status = if (Test-AccessDenied $_) { 'access-denied' } else { 'unavailable' }; exitCode = $null; text = '' }
  }
}

function New-EventXmlFailureRecord([string]$QueryName, $Event, [string]$ProviderName, [int]$EventId, [string]$Status = 'xml-conversion-failed') {
  return [pscustomobject]@{ query = $QueryName; provider = $ProviderName; id = $EventId; recordId = Get-SafeProperty $Event 'RecordId'; status = $Status }
}

function Get-SafeEventDetailsFromXml([string]$XmlText) {
  $details = [ordered]@{}
  $document = [xml]$XmlText
  $eventNode = Get-SafeProperty $document 'Event'
  if ($null -eq $eventNode) { throw 'Event XML has no Event node.' }
  $eventData = Get-SafeProperty $eventNode 'EventData'
  if ($null -eq $eventData) { return [pscustomobject]$details }
  foreach ($data in @(Get-SafeProperty $eventData 'Data' @())) {
    $name = [string](Get-SafeProperty $data 'Name')
    if ($name -in @('AppName','AppVersion','ModuleName','ModuleVersion','ExceptionCode','FaultingOffset','ProcessId','ServiceName')) { $details[$name] = Protect-Text ([string](Get-SafeProperty $data '#text')) }
  }
  return [pscustomobject]$details
}

function Get-ServiceAndEvents {
  $serviceName = 'TalkingQuillKeyboardAuthority'
  $needsElevation = $false
  try {
    $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'" -ErrorAction Stop
    $serviceRecord = if ($null -eq $service) { [pscustomobject]@{ name = $serviceName; installed = $false; expectedForCurrentRuntime = $false } } else {
      $failureQuery = Invoke-ScQuery 'qfailure' $serviceName
      $sidQuery = Invoke-ScQuery 'qsidtype' $serviceName
      $privilegeQuery = Invoke-ScQuery 'qprivs' $serviceName
      $scQueries = @($failureQuery, $sidQuery, $privilegeQuery)
      if (@($scQueries | Where-Object { (Get-SafeProperty $_ 'status') -eq 'access-denied' }).Count -gt 0) { $needsElevation = $true }
      $failureText = if ((Get-SafeProperty $failureQuery 'status') -eq 'ok') { [string](Get-SafeProperty $failureQuery 'text') } else { '' }
      $sidText = if ((Get-SafeProperty $sidQuery 'status') -eq 'ok') { [string](Get-SafeProperty $sidQuery 'text') } else { '' }
      $privilegeText = if ((Get-SafeProperty $privilegeQuery 'status') -eq 'ok') { [string](Get-SafeProperty $privilegeQuery 'text') } else { '' }
      [pscustomobject]@{
        name = Get-SafeProperty $service 'Name' $serviceName; installed = $true; state = Get-SafeProperty $service 'State'; status = Get-SafeProperty $service 'Status'; startMode = Get-SafeProperty $service 'StartMode'
        startAccount = Protect-AccountName ([string](Get-SafeProperty $service 'StartName'))
        processId = Get-SafeProperty $service 'ProcessId'; exitCode = Get-SafeProperty $service 'ExitCode'; serviceSpecificExitCode = Get-SafeProperty $service 'ServiceSpecificExitCode'; pathName = Protect-Text ([string](Get-SafeProperty $service 'PathName'))
        recovery = [pscustomobject]@{ restartConfigured = $failureText -match 'RESTART'; rebootConfigured = $failureText -match 'REBOOT'; runCommandConfigured = $failureText -match 'RUN PROCESS'; resetPeriodSeconds = if ($failureText -match '(?im)RESET_PERIOD[^:]*:\s*(\d+)') { [int64]$Matches[1] } else { $null } }
        serviceSidType = if ($sidText -match '(?im)SERVICE_SID_TYPE\s*:\s*(NONE|UNRESTRICTED|RESTRICTED)') { $Matches[1] } else { 'unavailable' }
        requiredPrivileges = @([regex]::Matches($privilegeText, '\bSe[A-Za-z]+Privilege\b') | ForEach-Object Value | Select-Object -Unique)
        scQueries = [pscustomobject]@{
          failureActions = [pscustomobject]@{ status = Get-SafeProperty $failureQuery 'status'; exitCode = Get-SafeProperty $failureQuery 'exitCode' }
          sidType = [pscustomobject]@{ status = Get-SafeProperty $sidQuery 'status'; exitCode = Get-SafeProperty $sidQuery 'exitCode' }
          privileges = [pscustomobject]@{ status = Get-SafeProperty $privilegeQuery 'status'; exitCode = Get-SafeProperty $privilegeQuery 'exitCode' }
        }
      }
    }
  } catch {
    if (Test-AccessDenied $_) { $needsElevation = $true }
    $serviceRecord = [pscustomobject]@{ name = $serviceName; unavailable = $true; reason = if ($needsElevation) { 'access-denied' } else { 'query-failed' } }
  }
  $eventRecords = @()
  $eventQueries = @()
  $eventXmlErrors = @()
  $eventXmlFailureCount = 0
  $start = (Get-Date).AddDays(-7)
  $queries = @(
    [pscustomobject]@{ name = 'system-service-control-manager'; log = 'System'; provider = 'Service Control Manager'; ids = @(7000,7001,7009,7011,7023,7024,7026,7031,7034,7035,7036,7040,7045) }
    [pscustomobject]@{ name = 'application-error'; log = 'Application'; provider = 'Application Error'; ids = @(1000) }
    [pscustomobject]@{ name = 'windows-error-reporting'; log = 'Application'; provider = 'Windows Error Reporting'; ids = @(1001) }
    [pscustomobject]@{ name = 'application-hang'; log = 'Application'; provider = 'Application Hang'; ids = @(1002) }
  )
  foreach ($query in $queries) {
    try {
      $queryLimit = 5000
      $events = @(Get-WinEvent -FilterHashtable @{ LogName = $query.log; ProviderName = $query.provider; Id = $query.ids; StartTime = $start } -MaxEvents $queryLimit -ErrorAction Stop)
      $queryXmlFailures = 0
      foreach ($event in $events) {
        $providerName = [string](Get-SafeProperty $event 'ProviderName')
        $eventId = ConvertTo-SafeInt (Get-SafeProperty $event 'Id') -1
        try { $xml = $event.ToXml() } catch {
          $queryXmlFailures++
          $eventXmlFailureCount++
          if ($eventXmlErrors.Count -lt 50) {
            $eventXmlErrors += New-EventXmlFailureRecord $query.name $event $providerName $eventId
          }
          continue
        }
        try { $details = Get-SafeEventDetailsFromXml $xml } catch {
          $queryXmlFailures++
          $eventXmlFailureCount++
          if ($eventXmlErrors.Count -lt 50) { $eventXmlErrors += New-EventXmlFailureRecord $query.name $event $providerName $eventId 'xml-parse-or-eventdata-failed' }
          continue
        }
        $component = if ($providerName -eq 'Service Control Manager' -and $xml -match '(?i)TalkingQuillKeyboardAuthority') { 'legacy-service' } elseif ($xml -match '(?i)(?:^|[^A-Za-z0-9-])talking-quill-keyboard-owner\.exe(?:[^A-Za-z0-9-]|$)') { 'keyboard-owner' } elseif ($xml -match '(?i)(?:^|[^A-Za-z0-9-])talking-quill-helper\.exe(?:[^A-Za-z0-9-]|$)') { 'gateway' } elseif ($xml -match '(?i)(?:^|[>\\])Talking Quill\.exe(?:[<"\\]|$)') { 'application' } else { $null }
        if ($null -eq $component) { continue }
        $eventRecords += [pscustomobject]@{ log = $query.log; timeCreatedUtc = ConvertTo-SafeUtcText (Get-SafeProperty $event 'TimeCreated'); id = $eventId; level = Get-SafeProperty $event 'LevelDisplayName'; provider = $providerName; recordId = Get-SafeProperty $event 'RecordId'; component = $component; safeDetails = $details }
      }
      $eventQueries += [pscustomobject]@{ name = $query.name; status = if ($queryXmlFailures -gt 0) { 'partial' } else { 'ok' }; returned = $events.Count; limitReached = ($events.Count -eq $queryLimit); xmlFailures = $queryXmlFailures }
    } catch {
      $noEvents = [string](Get-SafeProperty $_ 'FullyQualifiedErrorId') -match 'NoMatchingEventsFound'
      $denied = Test-AccessDenied $_
      if ($denied) { $needsElevation = $true }
      $eventQueries += [pscustomobject]@{ name = $query.name; status = if ($noEvents) { 'no-events' } elseif ($denied) { 'access-denied' } else { 'query-failed' }; returned = 0; limitReached = $false; xmlFailures = 0 }
    }
  }
  $failedQueries = @($eventQueries | Where-Object { (Get-SafeProperty $_ 'status') -notin @('ok','no-events') })
  $eventQuery = if ($failedQueries.Count -eq 0) { 'ok' } elseif ($failedQueries.Count -lt $eventQueries.Count) { 'partial' } elseif (@($failedQueries | Where-Object { (Get-SafeProperty $_ 'status') -eq 'access-denied' }).Count -gt 0) { 'access-denied' } else { 'query-failed' }
  $relevantEventCountBeforeCap = $eventRecords.Count
  $eventRecords = @($eventRecords | Sort-Object @{ Expression = { Get-SafeProperty $_ 'timeCreatedUtc' }; Descending = $true }, @{ Expression = { ConvertTo-SafeInt (Get-SafeProperty $_ 'recordId') }; Descending = $true } | Select-Object -First 200)
  return [pscustomobject]@{ elevationNeeded = $needsElevation; service = $serviceRecord; eventQuery = $eventQuery; eventQueries = $eventQueries; eventXmlFailureCount = $eventXmlFailureCount; eventXmlErrors = $eventXmlErrors; relevantEventCountBeforeCap = $relevantEventCountBeforeCap; recentRelevantEvents = $eventRecords }
}

function Initialize-ElevatedLauncher {
  if ('TqBoundedElevatedLauncher' -as [type]) { return }
  Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Threading;
public static class TqBoundedElevatedLauncher {
  public static Process Launch(string fileName, string arguments, int timeoutMilliseconds) {
    Process result = null;
    Exception failure = null;
    bool timedOut = false;
    object gate = new object();
    ManualResetEvent done = new ManualResetEvent(false);
    Thread thread = new Thread(() => {
      Process launched = null;
      try {
        ProcessStartInfo start = new ProcessStartInfo(fileName, arguments);
        start.UseShellExecute = true;
        start.Verb = "runas";
        launched = Process.Start(start);
        lock (gate) {
          if (timedOut && launched != null) { try { launched.Kill(); } catch { } }
          else { result = launched; }
        }
      } catch (Exception ex) { lock (gate) { failure = ex; } }
      finally { done.Set(); }
    });
    thread.IsBackground = true;
    thread.SetApartmentState(ApartmentState.STA);
    thread.Start();
    if (!done.WaitOne(timeoutMilliseconds)) {
      lock (gate) { timedOut = true; }
      throw new TimeoutException("Elevated worker launch timed out.");
    }
    done.Dispose();
    if (failure != null) { throw failure; }
    if (result == null) { throw new InvalidOperationException("Elevated worker launch returned no process."); }
    return result;
  }
}
'@
}

function Stop-ElevatedWorker([AllowNull()]$Worker) {
  if ($null -eq $Worker) { return }
  try {
    if (-not [bool](Get-SafeProperty $Worker 'HasExited' $false)) {
      $Worker.Kill()
      [void]$Worker.WaitForExit(5000)
    }
  } catch { }
}

function Read-BoundedPipePayload {
  param(
    [Parameter(Mandatory = $true)][IO.Pipes.NamedPipeServerStream]$Pipe,
    [int]$TimeoutMilliseconds = 30000,
    [int]$MaximumBytes = 1048576
  )
  $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
  $buffer = New-Object byte[] 4096
  $memory = [IO.MemoryStream]::new()
  try {
    while ($true) {
      $remaining = [int][Math]::Max(0, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
      if ($remaining -le 0) { throw 'Elevated detail payload read timed out.' }
      $pendingRead = $Pipe.BeginRead($buffer, 0, $buffer.Length, $null, $null)
      if (-not $pendingRead.AsyncWaitHandle.WaitOne($remaining)) { throw 'Elevated detail payload read timed out.' }
      $read = $Pipe.EndRead($pendingRead)
      if ($read -eq 0) { break }
      if ($memory.Length + $read -gt $MaximumBytes) { throw 'Elevated detail payload exceeded the size limit.' }
      $memory.Write($buffer, 0, $read)
    }
    return [Text.Encoding]::UTF8.GetString($memory.ToArray())
  } finally { $memory.Dispose() }
}

function Get-AuthenticatedElevatedPayload($Envelope, [string]$ExpectedCapability) {
  if ($null -eq $Envelope -or $null -eq $Envelope.PSObject.Properties['capability'] -or $null -eq $Envelope.PSObject.Properties['data']) { throw 'Elevated detail envelope is invalid.' }
  $actualCapability = [string](Get-SafeProperty $Envelope 'capability')
  if (-not (Test-CapabilityEqual $ExpectedCapability $actualCapability)) { throw 'Elevated detail capability authentication failed.' }
  return Get-SafeProperty $Envelope 'data'
}

function Test-ElevatedPayloadSchema($Value) {
  if ($null -eq $Value) { throw 'Elevated detail payload was null.' }
  foreach ($name in @('elevationNeeded','service','eventQuery','eventQueries','recentRelevantEvents')) {
    if ($null -eq $Value.PSObject.Properties[$name]) { throw "Elevated detail payload is missing $name." }
  }
  if ((Get-SafeProperty $Value 'elevationNeeded') -isnot [bool]) { throw 'Elevated detail payload has an invalid elevationNeeded value.' }
  if ($null -eq (Get-SafeProperty $Value 'service')) { throw 'Elevated detail payload has an invalid service value.' }
  if ([string](Get-SafeProperty $Value 'eventQuery') -notin @('ok','partial','access-denied','query-failed')) { throw 'Elevated detail payload has an invalid eventQuery.' }
  $events = @(Get-SafeProperty $Value 'recentRelevantEvents' @())
  if ($events.Count -gt 200) { throw 'Elevated detail payload has too many events.' }
  foreach ($event in $events) {
    if ([string](Get-SafeProperty $event 'component') -notin @('legacy-service','keyboard-owner','gateway','application')) { throw 'Elevated detail payload has an invalid event component.' }
    if ([string](Get-SafeProperty $event 'log') -notin @('System','Application')) { throw 'Elevated detail payload has an invalid event log.' }
  }
  $queryDetails = @(Get-SafeProperty $Value 'eventQueries' @())
  if ($queryDetails.Count -gt 4) { throw 'Elevated detail payload has too many event query results.' }
  foreach ($query in $queryDetails) {
    if ([string](Get-SafeProperty $query 'status') -notin @('ok','partial','no-events','access-denied','query-failed')) { throw 'Elevated detail payload has an invalid event query status.' }
  }
  $xmlErrors = @(Get-SafeProperty $Value 'eventXmlErrors' @())
  if ($xmlErrors.Count -gt 50 -or (ConvertTo-SafeInt (Get-SafeProperty $Value 'eventXmlFailureCount')) -lt $xmlErrors.Count) { throw 'Elevated detail payload has invalid event XML error data.' }
  return $true
}

function Set-ElevationAttempt($Value, [bool]$Succeeded, [AllowNull()]$Failure) {
  if ($null -eq $Value) { $Value = [pscustomobject]@{} }
  $attempt = [pscustomobject]@{ attempted = $true; succeeded = $Succeeded; failure = $Failure }
  $Value | Add-Member -NotePropertyName elevationAttempt -NotePropertyValue $attempt -Force
  return $Value
}

function Test-ZipIntegrity([string]$Path) {
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [IO.Compression.ZipFile]::OpenRead($Path)
  try {
    $entries = @($archive.Entries)
    if (@($entries.FullName | Group-Object | Where-Object Count -gt 1).Count -ne 0) { throw 'ZIP has duplicate entries.' }
    $manifestEntry = @($entries | Where-Object FullName -eq 'integrity.txt')
    if ($manifestEntry.Count -ne 1) { throw 'ZIP integrity manifest is missing or duplicated.' }
    $reader = [IO.StreamReader]::new($manifestEntry[0].Open(), [Text.Encoding]::UTF8)
    try { $lines = @($reader.ReadToEnd() -split "`r?`n" | Where-Object { $_ }) } finally { $reader.Dispose() }
    $expected = @{}
    foreach ($line in $lines) {
      if ($line -notmatch '^(?<hash>[0-9a-f]{64})  (?<name>[^/\\]+)$' -or $Matches.name -eq 'integrity.txt' -or $expected.ContainsKey($Matches.name)) { throw 'ZIP integrity manifest is invalid.' }
      $expected[$Matches.name] = $Matches.hash
    }
    if ($entries.Count -ne $expected.Count + 1) { throw 'ZIP entry count does not match integrity manifest.' }
    foreach ($entry in @($entries | Where-Object FullName -ne 'integrity.txt')) {
      if (-not $expected.ContainsKey($entry.FullName)) { throw "ZIP entry is not in integrity manifest: $($entry.FullName)" }
      $stream = $entry.Open()
      $sha = [Security.Cryptography.SHA256]::Create()
      try { $actual = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() } finally { $sha.Dispose(); $stream.Dispose() }
      if ($actual -ne $expected[$entry.FullName]) { throw "ZIP integrity check failed: $($entry.FullName)" }
    }
  } finally { $archive.Dispose() }
}

function Get-UniqueArchivePath([string]$Directory, [string]$Stamp) {
  $baseName = "Talking-Quill-diagnostics-$Stamp"
  $candidate = Join-Path $Directory "$baseName.zip"
  if (-not (Test-Path -LiteralPath $candidate)) { return $candidate }
  for ($attempt = 1; $attempt -le 100; $attempt++) {
    $candidate = Join-Path $Directory "$baseName-$('{0:D2}' -f $attempt).zip"
    if (-not (Test-Path -LiteralPath $candidate)) { return $candidate }
  }
  return Join-Path $Directory "$baseName-$([Guid]::NewGuid().ToString('N').Substring(0, 12)).zip"
}

function Complete-DiagnosticArchive {
  param(
    [Parameter(Mandatory = $true)][string]$StagingPath,
    [Parameter(Mandatory = $true)][string]$DestinationPath,
    [scriptblock]$FinalizationTestHook
  )
  if (-not (Test-Path -LiteralPath $StagingPath)) { throw 'Diagnostic staging directory is unavailable.' }
  $collectorCopy = Join-Path $StagingPath 'collector.ps1'
  if (-not (Test-Path -LiteralPath $collectorCopy)) { Copy-Item -LiteralPath $PSCommandPath -Destination $collectorCopy -ErrorAction Stop }
  $integrityPath = Join-Path $StagingPath 'integrity.txt'
  Remove-Item -LiteralPath $integrityPath -Force -ErrorAction SilentlyContinue
  $hashLines = @()
  foreach ($file in @(Get-ChildItem -LiteralPath $StagingPath -File -ErrorAction Stop | Sort-Object Name)) {
    $hash = [string](Get-SafeProperty (Get-FileHash -LiteralPath (Get-SafeProperty $file 'FullName') -Algorithm SHA256 -ErrorAction Stop) 'Hash')
    $hashLines += "$($hash.ToLowerInvariant())  $(Get-SafeProperty $file 'Name')"
  }
  [IO.File]::WriteAllLines($integrityPath, $hashLines, [Text.UTF8Encoding]::new($false))
  if (Test-Path -LiteralPath $DestinationPath) { throw 'Diagnostic ZIP destination already exists.' }
  try {
    Compress-Archive -Path (Join-Path $StagingPath '*') -DestinationPath $DestinationPath -CompressionLevel Optimal -ErrorAction Stop
    Set-CollectorArchiveAcl $DestinationPath
    if ($null -ne $FinalizationTestHook) { & $FinalizationTestHook }
    Test-ZipIntegrity $DestinationPath
  } catch {
    $finalizationFailure = $_
    if (Test-Path -LiteralPath $DestinationPath) { Remove-Item -LiteralPath $DestinationPath -Force -ErrorAction Stop }
    if (Test-Path -LiteralPath $DestinationPath) { throw 'Failed to delete an incomplete diagnostic ZIP.' }
    throw $finalizationFailure
  }
}

function Write-PartialDiagnosticArchive([string]$StagingPath, [string]$DestinationPath, $Failure) {
  if (-not (Test-Path -LiteralPath $StagingPath)) { New-Item -ItemType Directory -Path $StagingPath -Force -ErrorAction Stop | Out-Null }
  Write-JsonFile (Join-Path $StagingPath 'collector-failure.json') (New-SectionFailure 'collector' $Failure)
  $readmePath = Join-Path $StagingPath 'README.txt'
  if (-not (Test-Path -LiteralPath $readmePath)) {
    [IO.File]::WriteAllText($readmePath, "Talking Quill diagnostic collection failed before all sections completed.`r`nThe ZIP contains the safe partial results and collector-failure.json.`r`n", [Text.UTF8Encoding]::new($false))
  }
  Complete-DiagnosticArchive $StagingPath $DestinationPath
}

function Invoke-RedactionSelfTest {
  $sample = "user=$($script:UserName) path=$($script:UserProfile) sid=$($script:CurrentSid) token=abc123456789 password: swordfish https://me:pw@example.test/?api_key=hello"
  $clean = Protect-Text $sample
  foreach ($forbidden in @($script:UserProfile, $script:CurrentSid, 'abc123456789', 'swordfish', 'me:pw', 'hello')) {
    if (-not [string]::IsNullOrEmpty($forbidden) -and $clean.IndexOf($forbidden, [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "Redaction self-test leaked a protected value." }
  }
  if ($clean -match "(?i)user=$([regex]::Escape($script:UserName))(?:\s|$)") { throw 'Redaction self-test leaked the username.' }
  $object = Protect-Object ([pscustomobject]@{ accessToken = 'private'; safe = 'ok'; installationId = 'stable-installation-123'; tokenCount = 7 })
  if ($object.accessToken -ne '<REDACTED>' -or $object.safe -ne 'ok' -or $object.tokenCount -ne 7) { throw 'Object redaction self-test failed' }
  $stableId = Protect-StableIdentifier 'stable-installation-123'
  if ($object.installationId -ne $stableId -or $stableId -ne (Protect-StableIdentifier 'stable-installation-123') -or $stableId -eq (Protect-StableIdentifier 'other-installation')) { throw 'Stable installation identifier self-test failed.' }

  $quotedSecrets = @(
    '{"password": "secret value with spaces", "safe": 1}'
    "'api_key'='quoted secret value'"
    "Authorization: Bearer header-secret-value"
    'https://example.test/path?access_token=query-secret&safe=1'
    '--password "command secret with spaces"'
  )
  foreach ($secretText in $quotedSecrets) {
    $protectedSecretText = Protect-Text $secretText
    if ($protectedSecretText -match 'secret value|header-secret|query-secret|command secret') { throw 'Quoted/header/query credential redaction self-test failed.' }
  }
  $profileWithSpaces = Protect-Text 'open "C:\Users\Person With Spaces\Private File.txt" now'
  if ($profileWithSpaces -match 'Person With Spaces|Private File') { throw 'Quoted profile-path redaction self-test failed.' }
  $unquotedProfileWithSpaces = Protect-Text 'path=C:\Users\Person With Spaces\Private File.txt; code=1'
  if ($unquotedProfileWithSpaces -match 'Person With Spaces|Private File' -or $unquotedProfileWithSpaces -notmatch 'code=1') { throw 'Unquoted profile-path redaction self-test failed.' }
  $profileDifferentCase = Protect-Text $script:UserProfile.ToUpperInvariant()
  if ($profileDifferentCase -ne '<USERPROFILE>') { throw 'Case-insensitive profile-path redaction self-test failed.' }
  $unrelatedUserText = "prefix$($script:UserName)suffix"
  if ((Protect-Text $unrelatedUserText) -ne $unrelatedUserText) { throw 'Username boundary self-test corrupted unrelated text.' }
  $accountHash = Protect-AccountName "EXAMPLE\$($script:UserName)"
  if ($accountHash -notmatch '^<ACCOUNT:[0-9a-f]{16}>$' -or $accountHash -match [regex]::Escape($script:UserName)) { throw 'Account hashing self-test failed.' }
  $broaderSid = 'S-1-12-1-111111111-222222222-333333333-444444444'
  $protectedSid = Protect-Text "sid=$broaderSid intact=S-1-5-18 embedded=XS-1-12-1-1-2-3Y"
  if ($protectedSid -match [regex]::Escape($broaderSid) -or $protectedSid -notmatch 'intact=S-1-5-18' -or $protectedSid -notmatch 'embedded=XS-1-12-1-1-2-3Y') { throw 'Broader SID redaction boundary self-test failed.' }

  $validTimestamp = ConvertTo-ValidatedDiagnosticTimestamp '2024-01-02T03:04:05.123Z'
  $validEpochTimestamp = ConvertTo-ValidatedDiagnosticTimestamp ([DateTimeOffset]::Parse('2024-01-02T03:04:05.123Z').ToUnixTimeMilliseconds())
  if ($validTimestamp -ne '2024-01-02T03:04:05.1230000Z' -or $validEpochTimestamp -ne $validTimestamp -or $null -ne (ConvertTo-ValidatedDiagnosticTimestamp '01/02/2024 03:04:05') -or $null -ne (ConvertTo-ValidatedDiagnosticTimestamp 1) -or $null -ne (ConvertTo-ValidatedDiagnosticTimestamp '2999-01-01T00:00:00Z')) { throw 'Diagnostic timestamp validation self-test failed.' }
  $safeCommand = Get-SafeCommandLine '"C:\Users\Somebody\Talking Quill.exe" --type=renderer --diagnostic-capability=private --unknown=spoken-words raw-typed-text' 'Talking Quill.exe'
  if ($safeCommand -match 'Somebody|private|spoken-words|raw-typed-text') { throw 'Command-line redaction self-test failed.' }
  $emptyJson = ConvertTo-Json -InputObject (Protect-Object @()) -Compress
  $singleJson = ConvertTo-Json -InputObject (Protect-Object @('ok')) -Compress
  if ($emptyJson -ne '[]' -or $singleJson -ne '["ok"]') { throw 'JSON array preservation self-test failed.' }

  # This reproduces the registry failure that prompted the safe accessor: under
  # StrictMode, $missing.DisplayName throws PropertyNotFoundStrict.
  $missing = [pscustomobject]@{ Publisher = 'test' }
  $strictModeThrew = $false
  try { $null = $missing.DisplayName } catch { $strictModeThrew = $_.FullyQualifiedErrorId -match 'PropertyNotFoundStrict' }
  if (-not $strictModeThrew -or $null -ne (Get-SafeProperty $missing 'DisplayName')) { throw 'Missing-property self-test failed.' }
  if ((Get-SafeProperty $null 'Anything' 'fallback') -ne 'fallback') { throw 'Null-property self-test failed.' }
  if (@(@() | ForEach-Object { Get-SafeProperty $_ 'Name' }).Count -ne 0) { throw 'Empty-collection self-test failed.' }

  $denied = [Management.Automation.ErrorRecord]::new([UnauthorizedAccessException]::new('Access is denied.'), 'denied', [Management.Automation.ErrorCategory]::PermissionDenied, $null)
  if (-not (Test-AccessDenied $denied)) { throw 'Access-denied classification self-test failed.' }
  Initialize-ElevatedLauncher
  if (-not ('TqBoundedElevatedLauncher' -as [type])) { throw 'Bounded elevated launcher compilation self-test failed.' }
  $trustedPowerShell = Get-TrustedWindowsPowerShellPath
  if (-not [IO.Path]::IsPathRooted($trustedPowerShell) -or (Split-Path -Parent $trustedPowerShell).TrimEnd('\') -ine ([IO.Path]::GetFullPath($PSHOME).TrimEnd('\')) -or (Split-Path -Leaf $trustedPowerShell) -ine 'powershell.exe') { throw 'Trusted Windows PowerShell path self-test failed.' }

  $capability = Get-RandomCapability
  if ($capability -notmatch '^[0-9a-f]{64}$' -or -not (Test-CapabilityEqual $capability $capability) -or (Test-CapabilityEqual $capability ('0' * 64))) { throw 'Capability generation/comparison self-test failed.' }
  $capabilityPayload = [pscustomobject]@{ ok = $true }
  $authenticatedPayload = Get-AuthenticatedElevatedPayload ([pscustomobject]@{ capability = $capability; data = $capabilityPayload }) $capability
  if (-not [bool](Get-SafeProperty $authenticatedPayload 'ok' $false)) { throw 'Capability envelope self-test failed.' }
  try { [void](Get-AuthenticatedElevatedPayload ([pscustomobject]@{ capability = ('0' * 64); data = $capabilityPayload }) $capability); throw 'Incorrect capability was accepted.' } catch { if ($_.Exception.Message -eq 'Incorrect capability was accepted.') { throw } }
  if ((Protect-Text "capability=$capability -ElevatedCapability $capability") -match [regex]::Escape($capability) -or (Get-SafeProperty (Protect-Object ([pscustomobject]@{ capability = $capability })) 'capability') -ne '<REDACTED>' -or (Get-SafeProperty (Protect-Object ([pscustomobject]@{ ElevatedCapability = $capability })) 'ElevatedCapability') -ne '<REDACTED>') { throw 'Capability redaction self-test failed.' }

  $expectedInstallRoot = @(Get-ExpectedInstallRoots | Select-Object -First 1)[0]
  $trustedSystemDirectory = Get-TrustedSystemDirectory
  $originalSystemRoot = $env:SystemRoot
  $originalProgramFiles = $env:ProgramFiles
  try {
    $env:SystemRoot = 'Z:\untrusted-system-root'
    $env:ProgramFiles = 'Z:\untrusted-program-files'
    $mutatedInstallRoot = @(Get-ExpectedInstallRoots | Select-Object -First 1)[0]
    if ($mutatedInstallRoot -ne $expectedInstallRoot -or (Get-TrustedSystemDirectory) -ne $trustedSystemDirectory) { throw 'Environment-independent trust-anchor self-test failed.' }
  } finally { $env:SystemRoot = $originalSystemRoot; $env:ProgramFiles = $originalProgramFiles }
  if ($null -eq (Resolve-TrustedInstallRoot $expectedInstallRoot) -or $null -ne (Resolve-TrustedInstallRoot '\\server\share\Talking Quill') -or $null -ne (Resolve-TrustedInstallRoot '\\?\C:\Program Files\Talking Quill') -or $null -ne (Resolve-TrustedInstallRoot (Join-Path $trustedSystemDirectory 'Talking Quill'))) { throw 'Trusted install-root self-test failed.' }
  if ($null -ne (Resolve-TrustedInstalledPath $expectedInstallRoot '..\outside.exe')) { throw 'Installed-path traversal self-test failed.' }
  $pipeSecuritySelfTest = New-CollectorPipeSecurity
  $pipeRules = @($pipeSecuritySelfTest.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
  if ($pipeRules.Count -ne 1 -or [string](Get-SafeProperty (Get-SafeProperty $pipeRules[0] 'IdentityReference') 'Value') -ne $script:CurrentSid) { throw 'Current-user-only pipe ACL self-test failed.' }
  $scSelfTest = Invoke-ScQuery 'qc' "TalkingQuillSelfTestMissing$PID"
  if ((Get-SafeProperty $scSelfTest 'status') -eq 'ok' -or $null -eq (Get-SafeProperty $scSelfTest 'exitCode')) { throw 'sc.exe exit-code self-test failed.' }

  $projectRoot = Split-Path -Parent $PSScriptRoot
  $testRoot = Join-Path (Join-Path $projectRoot 'tmp') "Talking-Quill-selftest-$PID-$([Guid]::NewGuid().ToString('N'))"
  New-Item -ItemType Directory -Path $testRoot -Force -ErrorAction Stop | Out-Null
  try {
    $reparseTarget = Join-Path $testRoot 'reparse-target'
    $reparseLink = Join-Path $testRoot 'reparse-link'
    New-Item -ItemType Directory -Path $reparseTarget -ErrorAction Stop | Out-Null
    $cmdPath = Join-Path (Get-TrustedSystemDirectory) 'cmd.exe'
    & $cmdPath /d /c "mklink /J `"$reparseLink`" `"$reparseTarget`"" 2>$null | Out-Null
    $junctionExitCode = $LASTEXITCODE
    if ($junctionExitCode -ne 0) { throw 'Reparse-point self-test could not create a local junction.' }
    if (Test-NoReparsePointInPath $reparseLink) { throw 'Reparse-point rejection self-test failed.' }
    & $cmdPath /d /c "rmdir `"$reparseLink`"" 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Reparse-point self-test cleanup failed.' }

    $collisionStamp = '20000101-000000'
    $firstArchivePath = Get-UniqueArchivePath $testRoot $collisionStamp
    [IO.File]::WriteAllText($firstArchivePath, 'collision', [Text.UTF8Encoding]::new($false))
    $secondArchivePath = Get-UniqueArchivePath $testRoot $collisionStamp
    if ($firstArchivePath -eq $secondArchivePath -or (Split-Path -Leaf $secondArchivePath) -ne 'Talking-Quill-diagnostics-20000101-000000-01.zip') { throw 'Unique archive naming self-test failed.' }

    $failurePath = Join-Path $testRoot 'failed-section.json'
    $wroteSuccess = Write-CollectorSection $failurePath 'simulated-api' { throw [InvalidOperationException]::new("transport failed token=do-not-leak") }
    if ($wroteSuccess -or -not (Test-Path -LiteralPath $failurePath)) { throw 'Per-section failure self-test did not write output.' }
    $failureText = [IO.File]::ReadAllText($failurePath)
    if ($failureText -match 'do-not-leak' -or $failureText -notmatch 'REDACTED') { throw 'Per-section failure redaction self-test failed.' }
    $roundTrip = $failureText | ConvertFrom-Json
    if ((Get-SafeProperty $roundTrip 'available' $true) -ne $false) { throw 'Failure transport round-trip self-test failed.' }

    $validPayload = [pscustomobject]@{ elevationNeeded = $false; service = [pscustomobject]@{}; eventQuery = 'partial'; eventQueries = @([pscustomobject]@{ status = 'partial' }); eventXmlFailureCount = 1; eventXmlErrors = @([pscustomobject]@{ status = 'xml-conversion-failed' }); recentRelevantEvents = @() }
    if (-not (Test-ElevatedPayloadSchema $validPayload)) { throw 'Elevated payload schema self-test failed.' }
    $xmlFailureRecord = New-EventXmlFailureRecord 'self-test-query' ([pscustomobject]@{ RecordId = 42 }) 'self-test-provider' 1000
    if ((Get-SafeProperty $xmlFailureRecord 'status') -ne 'xml-conversion-failed' -or (Get-SafeProperty $xmlFailureRecord 'recordId') -ne 42) { throw 'Event XML failure recording self-test failed.' }
    try { [void](Get-SafeEventDetailsFromXml '<Event><EventData>'); throw 'Malformed Event XML was accepted.' } catch { if ($_.Exception.Message -eq 'Malformed Event XML was accepted.') { throw } }
    try { [void](Test-ElevatedPayloadSchema ([pscustomobject]@{ eventQuery = 'ok' })); throw 'Malformed elevated payload was accepted.' } catch { if ($_.Exception.Message -eq 'Malformed elevated payload was accepted.') { throw } }

    $finalizationStaging = Join-Path $testRoot 'finalization-staging'
    $failedFinalizationZip = Join-Path $testRoot 'failed-finalization.zip'
    New-Item -ItemType Directory -Path $finalizationStaging -ErrorAction Stop | Out-Null
    Write-JsonFile (Join-Path $finalizationStaging 'completed-section.json') ([pscustomobject]@{ ok = $true })
    $finalizationFailureObserved = $false
    try { Complete-DiagnosticArchive $finalizationStaging $failedFinalizationZip { throw 'injected finalization failure' } } catch { $finalizationFailureObserved = $_.Exception.Message -match 'injected finalization failure' }
    if (-not $finalizationFailureObserved -or (Test-Path -LiteralPath $failedFinalizationZip)) { throw 'Failed-finalization ZIP cleanup self-test failed.' }

    $partialStaging = Join-Path $testRoot 'partial-staging'
    $partialZip = Join-Path $testRoot 'partial.zip'
    New-Item -ItemType Directory -Path $partialStaging -ErrorAction Stop | Out-Null
    Write-JsonFile (Join-Path $partialStaging 'completed-section.json') ([pscustomobject]@{ ok = $true })
    try { throw [InvalidOperationException]::new('injected outer failure password=outer-secret') } catch { Write-PartialDiagnosticArchive $partialStaging $partialZip $_ }
    Test-ZipIntegrity $partialZip
    if (-not (Test-Path -LiteralPath $partialZip)) { throw 'Partial archive injection self-test failed.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $partialArchive = [IO.Compression.ZipFile]::OpenRead($partialZip)
    try {
      $failureEntry = @($partialArchive.Entries | Where-Object FullName -eq 'collector-failure.json')
      if ($failureEntry.Count -ne 1) { throw 'Partial archive failure record is missing.' }
      $failureReader = [IO.StreamReader]::new($failureEntry[0].Open())
      try { $partialFailureText = $failureReader.ReadToEnd() } finally { $failureReader.Dispose() }
      if ($partialFailureText -match 'outer-secret' -or $partialFailureText -notmatch 'REDACTED') { throw 'Partial archive failure redaction self-test failed.' }
    } finally { $partialArchive.Dispose() }
  } finally { Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue }
  Write-Output 'StrictMode, trusted paths, capability authentication, finalization cleanup, and recovery self-tests passed.'
}

if ($SelfTest) { Invoke-RedactionSelfTest; exit 0 }
if ($ElevatedWorker) {
  if ($ElevatedPipeName -notmatch '^TalkingQuill\.Diagnostics\.[0-9a-f]{32}$') { throw 'Elevated pipe name is invalid.' }
  if ($ElevatedCapability -notmatch '^[0-9a-f]{64}$') { throw 'Elevated diagnostic capability is invalid.' }
  $client = [IO.Pipes.NamedPipeClientStream]::new('.', $ElevatedPipeName, [IO.Pipes.PipeDirection]::Out)
  try {
    $client.Connect(30000)
    $writer = [IO.StreamWriter]::new($client, [Text.UTF8Encoding]::new($false), 4096, $true)
    $protectedData = Protect-Object (Get-ServiceAndEvents)
    $envelopeJson = ConvertTo-Json -InputObject ([pscustomobject]@{ capability = $ElevatedCapability; data = $protectedData }) -Depth 12
    try { $writer.Write($envelopeJson); $writer.Flush() } finally { $writer.Dispose() }
  } finally { $ElevatedCapability = $null; $client.Dispose() }
  exit 0
}

$outputRoot = $null
$stamp = $null
$zipPath = $null
$staging = $null
$archiveFailure = $null
$stagingSecured = $false

try {
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $outputRoot = Get-VerifiedOutputRoot $OutputDirectory
  $zipPath = Get-UniqueArchivePath $outputRoot $stamp
  $staging = Join-Path ([IO.Path]::GetTempPath()) "Talking-Quill-diagnostics-$PID-$stamp-$([Guid]::NewGuid().ToString('N'))"
  [void](Initialize-SecureStaging $staging)
  $stagingSecured = $true
  Write-Host 'Collecting Talking Quill diagnostics. Personal content is excluded.'
  $installCollectionFailure = $null
  $installRecords = try { @(Get-InstallRecords) } catch { $installCollectionFailure = New-SectionFailure 'installation-registry' $_; @() }
  $installQueryErrors = if ($null -ne $installCollectionFailure) { @($installCollectionFailure) } else { @($script:InstallQueryErrors) }
  $installRootCandidate = @(Get-ExpectedInstallRoots | Select-Object -First 1)[0]
  $installRoot = Resolve-TrustedInstallRoot $installRootCandidate
  $releaseManifestPath = if ($null -ne $installRoot) { Resolve-TrustedInstalledPath $installRoot 'resources\keyboard-owner-release-v1.json' } else { $null }
  $installedIdentity = [pscustomobject]@{ status = 'unavailable'; version = $null; architecture = $null; releaseBuildDigest = $null }
  if ($null -ne $releaseManifestPath) {
    try {
      $manifestFile = Get-Item -LiteralPath $releaseManifestPath -Force -ErrorAction Stop
      if (($manifestFile.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $manifestFile.Length -le 0 -or $manifestFile.Length -gt 65536) { throw 'Installed release manifest file is unsafe.' }
      $manifest = Get-Content -LiteralPath $releaseManifestPath -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
      $manifestVersion = [string](Get-SafeProperty $manifest 'version')
      $manifestArchitecture = [string](Get-SafeProperty $manifest 'architecture')
      $manifestDigest = [string](Get-SafeProperty $manifest 'releaseBuildDigest')
      if ((ConvertTo-SafeInt (Get-SafeProperty $manifest 'schemaVersion')) -ne 1 -or (Get-SafeProperty $manifest 'platform') -cne 'win' -or $manifestVersion -notmatch '^\d+\.\d+\.\d+$' -or $manifestArchitecture -notin @('x64','arm64') -or $manifestDigest -cnotmatch '^[0-9a-f]{64}$') { throw 'Installed release manifest identity is invalid.' }
      $installedIdentity = [pscustomobject]@{ status = 'validated-manifest'; version = $manifestVersion; architecture = $manifestArchitecture; releaseBuildDigest = $manifestDigest }
    } catch {
      $installedIdentity = [pscustomobject]@{ status = 'invalid-manifest'; version = $null; architecture = $null; releaseBuildDigest = $null }
    }
  }
  [void](Write-CollectorSection (Join-Path $staging 'system.json') 'system' {
    $operatingSystem = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
    $computerSystem = Get-CimInstance Win32_ComputerSystem -ErrorAction Stop
    [pscustomobject]@{
      collectorVersion = $script:CollectorVersion
      collectedUtc = [DateTime]::UtcNow.ToString('o')
      installedAppVersion = $installedIdentity.version
      installedArchitecture = $installedIdentity.architecture
      installedIdentityStatus = $installedIdentity.status
      windows = [pscustomobject]@{ caption = Get-SafeProperty $operatingSystem 'Caption'; version = Get-SafeProperty $operatingSystem 'Version'; buildNumber = Get-SafeProperty $operatingSystem 'BuildNumber'; osArchitecture = Get-SafeProperty $operatingSystem 'OSArchitecture' }
      computerSystem = [pscustomobject]@{ systemType = Get-SafeProperty $computerSystem 'SystemType' }
      processArchitecture = $env:PROCESSOR_ARCHITECTURE
      osArchitecture = $env:PROCESSOR_ARCHITEW6432
      sessionId = Get-SafeProperty (Get-Process -Id $PID -ErrorAction Stop) 'SessionId'
      integrity = (& { Initialize-TokenInspector; [TqTokenInspector]::Integrity($PID) })
      userSidHash = $script:SidHash
    }
  })
  [void](Write-CollectorSection (Join-Path $staging 'installation.json') 'installation' {
    $publicInstallRecords = @($installRecords | Select-Object hive,registryViews,keyNameHash,displayName,displayVersion,publisher,installLocation,installLocationStatus,displayIcon,installDate,estimatedSizeKiB,noModify,noRepair)
    [pscustomobject]@{ registry = $publicInstallRecords; registryErrors = $installQueryErrors; files = @(Get-InstalledFiles $installRecords) }
  })
  [void](Write-CollectorSection (Join-Path $staging 'processes-before.json') 'processes-before' { @(Get-TalkingQuillProcesses) })

  $programDataRoot = Join-Path $env:ProgramData 'Talking Quill\KeyboardAuthority'
  $helperRoot = if ($null -ne $installRoot) { Resolve-TrustedInstalledPath $installRoot 'resources\helper' } else { $null }
  $userData = Join-Path $env:APPDATA 'Talking Quill'
  [void](Write-CollectorSection (Join-Path $staging 'manifest-summary.json') 'manifest-summary' {
    @(
      Get-ManifestSummary (Join-Path $programDataRoot 'enrollment-v1.json')
      if ($null -ne $releaseManifestPath) { Get-ManifestSummary $releaseManifestPath } else { [pscustomobject]@{ path = Protect-Text $installRootCandidate; exists = $false; error = 'rejected-untrusted-or-reparse-path' } }
    )
  })
  [void](Write-CollectorSection (Join-Path $staging 'acls.json') 'acls' {
    @(
      if ($null -ne $installRoot) { Get-AclSummary $installRoot } else { [pscustomobject]@{ path = Protect-Text $installRootCandidate; exists = $false; error = 'rejected-untrusted-or-reparse-path' } }
      if ($null -ne $helperRoot) { Get-AclSummary $helperRoot } else { [pscustomobject]@{ path = Protect-Text $installRootCandidate; exists = $false; error = 'rejected-untrusted-or-reparse-path' } }
      Get-AclSummary $programDataRoot
      Get-AclSummary $userData
      Get-AclSummary (Join-Path $userData 'logs')
    )
  })
  [void](Write-CollectorSection (Join-Path $staging 'settings-models-metadata.json') 'settings-models-metadata' {
    $modelsPath = Join-Path $userData 'models'
    [pscustomobject]@{
      statement = 'Contents were not read or copied.'
      settings = Get-FileMetadata (Join-Path $userData 'settings.json')
      modelsRoot = Get-FileMetadata $modelsPath
      modelEntries = if (Test-Path -LiteralPath $modelsPath -ErrorAction SilentlyContinue) {
        @(Get-ChildItem -LiteralPath $modelsPath -Force -ErrorAction Stop | Select-Object -First 200 | ForEach-Object {
          $isContainer = [bool](Get-SafeProperty $_ 'PSIsContainer' $false)
          [pscustomobject]@{ nameHash = Get-ShortHash ([string](Get-SafeProperty $_ 'Name')); type = if ($isContainer) { 'directory' } else { 'file' }; length = if ($isContainer) { $null } else { Get-SafeProperty $_ 'Length' }; modifiedUtc = ConvertTo-SafeUtcText (Get-SafeProperty $_ 'LastWriteTimeUtc') }
        })
      } else { @() }
    }
  })

  $diagnostics = try { Get-DiagnosticTail (Join-Path $userData 'logs') } catch { New-SectionFailure 'diagnostic-tail' $_ }
  [void](Write-CollectorSection (Join-Path $staging 'diagnostic-tail.json') 'diagnostic-tail' { $diagnostics })
  [void](Write-CollectorSection (Join-Path $staging 'protocol-readiness.json') 'protocol-readiness' {
    $processSnapshot = @(Get-TalkingQuillProcesses)
    [pscustomobject]@{
      runtimeIdentity = [pscustomobject]@{ version = $installedIdentity.version; architecture = $installedIdentity.architecture; status = $installedIdentity.status; topology = 'Electron -> gateway -> keyboard owner'; legacyAuthorityExpected = $false }
      publicLocalDiagnosticEndpoint = 'none in the production personal runtime; credential-bearing private pipes were not contacted'
      readinessEvidence = if ((ConvertTo-SafeInt (Get-SafeProperty $diagnostics 'acceptedEntryCount')) -gt 0) { 'See diagnostic-tail.json helper.readiness.changed and helper.runtime.snapshot entries.' } else { 'No persisted readiness entries were available. See 60-second-process-monitor.json.' }
      gatewayProcesses = @($processSnapshot | Where-Object { (Get-SafeProperty $_ 'name') -eq 'talking-quill-helper.exe' } | Select-Object name,pid,parentPid,sessionId,integrity)
      ownerProcesses = @($processSnapshot | Where-Object { (Get-SafeProperty $_ 'name') -eq 'talking-quill-keyboard-owner.exe' } | Select-Object name,pid,parentPid,sessionId,integrity)
    }
  })

  $serviceEventsPath = Join-Path $staging 'service-and-events.json'
  $serviceEvents = try { Get-ServiceAndEvents } catch { New-SectionFailure 'service-and-events' $_ }
  if ([bool](Get-SafeProperty $serviceEvents 'elevationNeeded' $false)) {
    Write-Host 'Windows denied service/event details. A UAC prompt will request those details only.'
    $worker = $null
    $server = $null
    try {
      $pipeName = "TalkingQuill.Diagnostics.$([Guid]::NewGuid().ToString('N'))"
      $elevatedCapability = Get-RandomCapability
      $pipeSecurity = New-CollectorPipeSecurity
      $server = [IO.Pipes.NamedPipeServerStream]::new($pipeName, [IO.Pipes.PipeDirection]::In, 1, [IO.Pipes.PipeTransmissionMode]::Byte, [IO.Pipes.PipeOptions]::Asynchronous, 4096, 262144, $pipeSecurity)
      Initialize-ElevatedLauncher
      $windowsPowerShellPath = Get-TrustedWindowsPowerShellPath
      $escapedScriptPath = $PSCommandPath.Replace('"', '\"')
      $launchArguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$escapedScriptPath`" -ElevatedWorker -ElevatedPipeName $pipeName -ElevatedCapability $elevatedCapability"
      $worker = [TqBoundedElevatedLauncher]::Launch($windowsPowerShellPath, $launchArguments, 30000)
      $pendingConnection = $server.BeginWaitForConnection($null, $null)
      $waitHandle = Get-SafeProperty $pendingConnection 'AsyncWaitHandle'
      if ($null -eq $waitHandle -or -not $waitHandle.WaitOne(30000)) { throw 'Elevated detail query connection timed out.' }
      $server.EndWaitForConnection($pendingConnection)
      $elevatedJson = Read-BoundedPipePayload $server 30000 1048576
      if (-not $worker.WaitForExit(10000)) {
        Stop-ElevatedWorker $worker
        throw 'Elevated detail query worker exit timed out.'
      }
      if ((ConvertTo-SafeInt (Get-SafeProperty $worker 'ExitCode') -1) -ne 0 -or [string]::IsNullOrWhiteSpace($elevatedJson)) { throw 'Elevated detail query did not complete.' }
      $envelope = $elevatedJson | ConvertFrom-Json -ErrorAction Stop
      $validated = Get-AuthenticatedElevatedPayload $envelope $elevatedCapability
      [void](Test-ElevatedPayloadSchema $validated)
      $elevatedCapability = $null
      $validated = Set-ElevationAttempt $validated $true $null
      [void](Write-CollectorSection $serviceEventsPath 'service-and-events-elevated' { $validated })
    } catch {
      Stop-ElevatedWorker $worker
      $elevationFailure = New-SectionFailure 'elevated-service-and-events' $_
      $serviceEvents = Set-ElevationAttempt $serviceEvents $false $elevationFailure
      [void](Write-CollectorSection $serviceEventsPath 'service-and-events' { $serviceEvents })
    } finally { $elevatedCapability = $null; if ($null -ne $server) { $server.Dispose() } }
  } else {
    $serviceEvents | Add-Member -NotePropertyName elevationAttempt -NotePropertyValue ([pscustomobject]@{ attempted = $false; succeeded = $false; failure = $null }) -Force
    [void](Write-CollectorSection $serviceEventsPath 'service-and-events' { $serviceEvents })
  }

  if ((ConvertTo-SafeInt (Get-SafeProperty $diagnostics 'acceptedEntryCount')) -eq 0) {
    Write-Host 'The diagnostic log is empty. Watching relevant child starts/exits for 60 seconds...'
    [void](Write-CollectorSection (Join-Path $staging '60-second-process-monitor.json') 'process-monitor' { Monitor-TalkingQuillExits 60 })
  } else {
    [void](Write-CollectorSection (Join-Path $staging '60-second-process-monitor.json') 'process-monitor' { [pscustomobject]@{ durationSeconds = 0; skipped = $true; reason = 'Persisted bounded diagnostic entries were available.' } })
  }
  [void](Write-CollectorSection (Join-Path $staging 'processes-after.json') 'processes-after' { @(Get-TalkingQuillProcesses) })

  $readme = @"
Talking Quill Windows diagnostic bundle

Collector version: $($script:CollectorVersion)
Collected UTC: $([DateTime]::UtcNow.ToString('o'))
Installed version: Talking Quill $(if ($null -ne $installedIdentity.version) { $installedIdentity.version } else { 'unavailable' })
Installed identity status: $($installedIdentity.status)

This bundle contains app/install metadata, hashes, relevant process and service state, bounded relevant Windows events, ACL summaries, manifest summaries, readiness evidence, and a bounded diagnostic tail. When persisted diagnostics were empty, the collector watched only Talking Quill process start/exit events for up to 60 seconds.

The collector does not intentionally read or copy recordings, model bytes, settings contents, voice commands, typed keys, history, or screenshots. It filters process and event data to Talking Quill and redacts recognized credentials, profile paths, account names, and non-well-known account SIDs. Diagnostic metadata can still identify software configuration, so review the JSON files before sharing.

integrity.txt lists SHA-256 hashes for every other file in this ZIP, including a copy of the collector.
"@
  [IO.File]::WriteAllText((Join-Path $staging 'README.txt'), $readme, [Text.UTF8Encoding]::new($false))

  Complete-DiagnosticArchive $staging $zipPath
  Write-Host ''
  Write-Host "Diagnostic ZIP created: $zipPath"
} catch {
  $outerFailure = $_
  Write-Warning "Collection did not complete; attempting a privacy-safe partial ZIP: $(Protect-Text $_.Exception.Message)"
  try {
    if ([string]::IsNullOrWhiteSpace($stamp)) { $stamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') }
    if (-not $stagingSecured) {
      if (-not [string]::IsNullOrWhiteSpace($staging)) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
      $staging = Join-Path ([IO.Path]::GetTempPath()) "Talking-Quill-diagnostics-recovery-$PID-$stamp-$([Guid]::NewGuid().ToString('N'))"
      [void](Initialize-SecureStaging $staging)
      $stagingSecured = $true
    }
    $perUserRecoveryRoot = Get-VerifiedRecoveryOutputRoot
    $recoveryRoots = @(@($outputRoot, $perUserRecoveryRoot) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    $partialCreated = $false
    $lastArchiveError = $null
    foreach ($recoveryRoot in $recoveryRoots) {
      try {
        if (-not (Test-Path -LiteralPath $recoveryRoot)) { New-Item -ItemType Directory -Path $recoveryRoot -Force -ErrorAction Stop | Out-Null }
        $zipPath = Get-UniqueArchivePath $recoveryRoot $stamp
        Write-PartialDiagnosticArchive $staging $zipPath $outerFailure
        $partialCreated = $true
        break
      } catch { $lastArchiveError = $_ }
    }
    if (-not $partialCreated) {
      if ($null -ne $lastArchiveError) { throw $lastArchiveError }
      throw 'No diagnostic ZIP recovery destination was available.'
    }
    Write-Host ''
    Write-Host "Partial diagnostic ZIP created: $zipPath"
  } catch { $archiveFailure = $_ }
} finally {
  if (-not [string]::IsNullOrWhiteSpace($staging)) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
}
if ($null -ne $archiveFailure) { throw $archiveFailure }
