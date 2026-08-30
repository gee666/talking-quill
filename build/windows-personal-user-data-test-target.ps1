param(
  [Parameter(Mandatory = $true)]
  [string]$Root,

  [Parameter(Mandatory = $true)]
  [string]$OutputFile
)

$ErrorActionPreference = 'Stop'

$localAppData = [IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd('\')
$testBase = [IO.Path]::GetFullPath((Join-Path $localAppData 'Temp\TQTests')).TrimEnd('\')
$rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\')
if (-not $rootPath.StartsWith("$testBase\", [StringComparison]::OrdinalIgnoreCase)) {
  throw 'The isolated uninstall root is outside the fixed current-user test folder.'
}
$relativeRoot = $rootPath.Substring($testBase.Length + 1)
if ([string]::IsNullOrWhiteSpace($relativeRoot) -or $relativeRoot.Contains('\')) {
  throw 'The isolated uninstall root must be one direct child of the fixed test folder.'
}
$target = [IO.Path]::GetFullPath(
  (Join-Path $rootPath 'profile\AppData\Roaming\Talking Quill')
).TrimEnd('\')
$expected = "$rootPath\profile\AppData\Roaming\Talking Quill"
if ($target -cne $expected) { throw 'The isolated Talking Quill test path is not canonical.' }

$current = $localAppData
foreach ($child in @('Temp', 'TQTests', $relativeRoot, 'profile', 'AppData', 'Roaming', 'Talking Quill')) {
  if (Test-Path -LiteralPath $current) {
    $item = Get-Item -Force -LiteralPath $current
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing redirected isolated uninstall test path: $current"
    }
  }
  $current = Join-Path $current $child
}
if (Test-Path -LiteralPath $target) {
  $item = Get-Item -Force -LiteralPath $target
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing redirected isolated Talking Quill test path: $target"
  }
}

$source = "[Target]`r`nPath=$target`r`n"
[IO.File]::WriteAllText($OutputFile, $source, [Text.UnicodeEncoding]::new($false, $true))
