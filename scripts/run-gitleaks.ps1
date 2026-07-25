$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Version = '8.28.0'
$ExpectedSha256 = 'DA6458E8864AF553807DE1C46A7A8EAC0880BD6B99BA56288E87E86A45AF884F'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ToolDirectory = Join-Path $Root 'tmp/security/gitleaks-8.28.0'
$Archive = Join-Path $ToolDirectory 'gitleaks.zip'
$Executable = Join-Path $ToolDirectory 'gitleaks.exe'
New-Item -ItemType Directory -Path $ToolDirectory -Force | Out-Null

if (-not (Test-Path $Executable)) {
  Invoke-WebRequest -UseBasicParsing "https://github.com/gitleaks/gitleaks/releases/download/v$Version/gitleaks_${Version}_windows_x64.zip" -OutFile $Archive
  $Actual = (Get-FileHash $Archive -Algorithm SHA256).Hash
  if ($Actual -ne $ExpectedSha256) {
    throw "Pinned gitleaks archive checksum mismatch: $Actual"
  }
  Expand-Archive -Path $Archive -DestinationPath $ToolDirectory -Force
}

$ReportedVersion = (& $Executable version).Trim()
if ($ReportedVersion -ne $Version) {
  throw "Expected gitleaks $Version, received $ReportedVersion"
}
Push-Location $Root
try {
  & $Executable git . --log-opts='--all' --config='.gitleaks.toml' --redact --no-banner
  if ($LASTEXITCODE -ne 0) { throw "gitleaks full-history scan failed with $LASTEXITCODE" }
} finally {
  Pop-Location
}
