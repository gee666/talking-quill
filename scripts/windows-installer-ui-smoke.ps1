param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$Provenance,
    [Parameter(Mandatory = $true)][ValidateSet('x64', 'arm64')][string]$Architecture,
    [Parameter(Mandatory = $true)][string]$Output,
    [ValidateRange(5, 120)][int]$TimeoutSeconds = 30
)
$ErrorActionPreference = [Management.Automation.ActionPreference]::Stop
if (-not $IsWindows -and $env:OS -ne 'Windows_NT') { throw 'The installer UI smoke gate requires Windows.' }
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The disposable installer UI smoke gate must run elevated.'
}
$nativeArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$expectedNativeArchitecture = if ($Architecture -ceq 'arm64') { 'Arm64' } else { 'X64' }
if ($nativeArchitecture -cne $expectedNativeArchitecture) {
    throw "Installer UI smoke requires native $Architecture Windows, found $nativeArchitecture."
}
$root = [IO.Path]::GetFullPath((Get-Location).Path)
$tmp = [IO.Path]::GetFullPath((Join-Path $root 'tmp')) + [IO.Path]::DirectorySeparatorChar
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$provenancePath = (Resolve-Path -LiteralPath $Provenance).Path
$outputPath = [IO.Path]::GetFullPath($Output)
if (-not $outputPath.StartsWith($tmp, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Installer UI evidence output must be under project tmp/.'
}
$manifest = Get-Content -Raw -LiteralPath $provenancePath | ConvertFrom-Json
$final = @($manifest.entries | Where-Object role -eq 'final-artifact')
if ($manifest.schemaVersion -ne 2 -or $manifest.package.platform -ne 'win' -or
    $manifest.package.arch -ne $Architecture -or $manifest.sourceCommit -cnotmatch '^[0-9a-f]{40}$' -or
    $manifest.sourceTree -cnotmatch '^[0-9a-f]{40}$' -or
    $manifest.sourceTreeSha256 -cnotmatch '^[0-9a-f]{64}$' -or $final.Count -ne 1 -or
    [IO.Path]::GetFileName([string]$final[0].path) -cne [IO.Path]::GetFileName($installerPath) -or
    [string]$final[0].sha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw 'Installer UI smoke provenance binding is invalid.'
}
$actualHash = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
$provenanceDocumentHash = (Get-FileHash -LiteralPath $provenancePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -cne [string]$final[0].sha256) { throw 'Installer bytes do not match provenance.' }
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($outputPath)) | Out-Null
$observer = Join-Path $tmp ('windows-installer-ui-observer-' + $PID + '.exe')
$csc = Join-Path ([Environment]::GetFolderPath('Windows')) 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
try {
    & $csc /nologo /target:winexe /reference:System.Management.dll "/out:$observer" (Join-Path $root 'scripts/windows-installer-ui-observer.cs')
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile the installer UI observer.' }
    $arguments = @(
        '"' + $installerPath + '"',
        '"' + $outputPath + '"',
        [string]($TimeoutSeconds * 1000),
        $Architecture,
        [string]$manifest.sourceCommit,
        [string]$manifest.sourceTree,
        [string]$manifest.sourceTreeSha256,
        $actualHash,
        $provenanceDocumentHash
    )
    $result = Start-Process -FilePath $observer -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
    if ($result.ExitCode -ne 0) { throw "Installer UI observer failed with exit code $($result.ExitCode)." }
    node scripts/windows-installer-ui-evidence.mjs --evidence $outputPath --installer $installerPath --provenance $provenancePath --arch $Architecture
    if ($LASTEXITCODE -ne 0) { throw 'Installer UI evidence validation failed.' }
} finally {
    Remove-Item -LiteralPath $observer -Force -ErrorAction SilentlyContinue
}
