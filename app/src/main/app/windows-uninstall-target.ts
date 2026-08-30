import { spawnSync } from 'node:child_process';
import type { SpawnSyncOptionsWithStringEncoding, SpawnSyncReturns } from 'node:child_process';
import { resolve } from 'node:path';

const TARGET_SCRIPT = String.raw`
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
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
if ($sids.Count -ne 1 -or $sids[0] -notmatch '^S-1-5-21-(?:[0-9]+-){3}[0-9]+$') { exit 70 }
$profileValue = Get-ItemPropertyValue -LiteralPath (
  "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$($sids[0])"
) -Name ProfileImagePath
$profile = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($profileValue)).TrimEnd('\')
if (-not (Test-Path -LiteralPath $profile -PathType Container)) { exit 70 }
$current = $profile
foreach ($child in @('AppData', 'Roaming', 'Talking Quill')) {
  if (Test-Path -LiteralPath $current) {
    $item = Get-Item -Force -LiteralPath $current
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { exit 70 }
  }
  $current = Join-Path $current $child
}
$target = [IO.Path]::GetFullPath((Join-Path $profile 'AppData\Roaming\Talking Quill')).TrimEnd('\')
if ($target -cne "$profile\AppData\Roaming\Talking Quill") { exit 70 }
[Console]::Write($target)
`;

export type TextSpawn = (
  command: string,
  arguments_: readonly string[],
  options: SpawnSyncOptionsWithStringEncoding,
) => SpawnSyncReturns<string>;

export function resolveSignedInWindowsUserDataTarget(
  run: TextSpawn = spawnSync,
  environment: NodeJS.ProcessEnv = process.env,
): string {
  const systemRoot = environment.SystemRoot ?? environment.WINDIR;
  if (!systemRoot) throw new Error('Windows system root is unavailable');
  const executable = resolve(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const result = run(
    executable,
    ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', TARGET_SCRIPT],
    { encoding: 'utf8', windowsHide: true, env: environment },
  );
  const target = result.status === 0 ? result.stdout.trim() : '';
  if (!target) throw new Error('Could not resolve the signed-in Windows user data target');
  return target;
}
