param(
  [Parameter(Mandatory = $true)]
  [string]$OutputFile
)

$ErrorActionPreference = 'Stop'

$sessionId = (Get-Process -Id $PID).SessionId
$sids = @(Get-Process explorer -ErrorAction SilentlyContinue |
  Where-Object SessionId -eq $sessionId |
  ForEach-Object {
    $record = Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)"
    $owner = Invoke-CimMethod -InputObject $record -MethodName GetOwner
    if ($owner.ReturnValue -eq 0) {
      ([Security.Principal.NTAccount]("$($owner.Domain)\$($owner.User)")).Translate(
        [Security.Principal.SecurityIdentifier]
      ).Value
    }
  } | Sort-Object -Unique)
if ($sids.Count -ne 1) {
  throw 'Could not identify one signed-in Windows desktop owner for confirmed personal-data removal.'
}
$sid = $sids[0]
if ($sid -notmatch '^S-1-5-21-(?:[0-9]+-){3}[0-9]+$') {
  throw 'The signed-in Windows desktop is not a normal user profile.'
}
$profileValue = Get-ItemPropertyValue -LiteralPath (
  "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid"
) -Name ProfileImagePath
$profile = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($profileValue)).TrimEnd('\')
if (-not (Test-Path -LiteralPath $profile -PathType Container)) {
  throw 'The signed-in Windows profile folder is unavailable.'
}
$target = [IO.Path]::GetFullPath((Join-Path $profile 'AppData\Roaming\Talking Quill')).TrimEnd('\')
$expected = "$profile\AppData\Roaming\Talking Quill"
if ($target -cne $expected) { throw 'The Talking Quill personal-data path is not canonical.' }

$current = $profile
foreach ($child in @('AppData', 'Roaming', 'Talking Quill')) {
  if (Test-Path -LiteralPath $current) {
    $item = Get-Item -Force -LiteralPath $current
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing redirected signed-in user profile path: $current"
    }
  }
  $current = Join-Path $current $child
}
if (Test-Path -LiteralPath $target) {
  $item = Get-Item -Force -LiteralPath $target
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing redirected Talking Quill personal-data path: $target"
  }
}

$source = "[Target]`r`nPath=$target`r`n"
[IO.File]::WriteAllText($OutputFile, $source, [Text.UnicodeEncoding]::new($false, $true))
