import { constants } from 'node:fs';
import { mkdir, mkdtemp, open, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));

export const WINDOWS_STAGED_PATH_ENV = 'TALKING_QUILL_PERSONAL_STAGED_INSTALLER';
export const WINDOWS_STAGED_SHA256_ENV = 'TALKING_QUILL_PERSONAL_STAGED_SHA256';
const WINDOWS_STAGING_CLEANUP_ATTEMPTS = 10;

export const WINDOWS_ELEVATION_WRAPPER = [
  '$ErrorActionPreference="Stop"',
  '$stream=$null',
  '$hasher=$null',
  'try{',
  '$path=[Environment]::GetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_INSTALLER","Process")',
  '$expected=[Environment]::GetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_SHA256","Process")',
  'if([string]::IsNullOrWhiteSpace($path)-or[string]::IsNullOrWhiteSpace($expected)-or$expected-cnotmatch "^[0-9a-f]{64}$"){exit 70}',
  '[Environment]::SetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_INSTALLER",$null,"Process")',
  '[Environment]::SetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_SHA256",$null,"Process")',
  '$stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)',
  '$hasher=[Security.Cryptography.SHA256]::Create()',
  '$actual=-join($hasher.ComputeHash($stream)|ForEach-Object{$_.ToString("x2")})',
  '$hasher.Dispose()',
  '$hasher=$null',
  'if($actual-cne$expected){exit 70}',
  '$process=Start-Process -FilePath $path -PassThru -ErrorAction Stop',
  'if($null-eq$process){exit 70}',
  '$process.WaitForExit()',
  'exit $process.ExitCode',
  '}catch{exit 70}',
  'finally{if($null-ne$hasher){$hasher.Dispose()};if($null-ne$stream){$stream.Dispose()}}',
].join('\n');

export async function removeWindowsInstallerStaging(
  staging,
  remove = rm,
  wait = delay,
  attempts = WINDOWS_STAGING_CLEANUP_ATTEMPTS,
) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await remove(staging, { recursive: true, force: true });
      return;
    } catch (error) {
      lastError = error;
      if (attempt < attempts) await wait(attempt * 100);
    }
  }
  throw new Error('Windows installer staging cleanup failed after bounded retries', {
    cause: lastError,
  });
}

export async function withStagedWindowsInstaller(
  checked,
  launch,
  stagingParent = resolve(root, 'tmp'),
  cleanup = removeWindowsInstallerStaging,
) {
  await mkdir(stagingParent, { recursive: true, mode: 0o700 });
  const staging = await mkdtemp(resolve(stagingParent, 'personal-use-win-install-'));
  const stagedInstaller = resolve(staging, 'checked-package.exe');
  let stagedHandle;
  let primaryError;
  let launchResult;
  try {
    stagedHandle = await open(
      stagedInstaller,
      constants.O_CREAT | constants.O_EXCL | constants.O_RDWR | constants.O_NOFOLLOW,
      0o600,
    );
    await stagedHandle.writeFile(checked.artifactBytes);
    await stagedHandle.sync();
    await stagedHandle.close();
    stagedHandle = undefined;
    // The launcher must verify through a retained read-only FileStream and keep
    // that same object open until the elevated installer process has exited.
    launchResult = await launch(stagedInstaller, checked.sha256);
  } catch (error) {
    primaryError = error;
  }
  await stagedHandle?.close().catch(() => undefined);
  let cleanupError;
  try {
    await cleanup(staging);
  } catch (error) {
    cleanupError = error;
  }
  if (primaryError !== undefined) {
    if (cleanupError !== undefined) {
      console.error(
        'Windows installer staging cleanup also failed; the primary install error is retained.',
      );
    }
    throw primaryError;
  }
  if (cleanupError !== undefined) throw cleanupError;
  return launchResult;
}
