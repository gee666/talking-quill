import { randomBytes } from 'node:crypto';
import { readFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { writeJsonAtomic } from '../persistence/atomic-json';

const INTENT_NAME = 'windows-update-relaunch-intent-v1.json';

export interface WindowsUpdateRelaunchIntent {
  readonly schemaVersion: 1;
  readonly nonce: string;
  readonly sourceVersion: string;
  readonly targetVersion: string;
  readonly phase: 'armed' | 'setup-started' | 'setup-complete' | 'launch-started' | 'app-ready';
  readonly completedVersion: string | null;
}

export async function createWindowsUpdateRelaunchIntent(
  userDataRoot: string,
  sourceVersion: string,
  targetVersion: string,
): Promise<{ readonly path: string; readonly intent: WindowsUpdateRelaunchIntent }> {
  const root = resolve(userDataRoot);
  const path = join(root, INTENT_NAME);
  const intent: WindowsUpdateRelaunchIntent = {
    schemaVersion: 1,
    nonce: randomBytes(16).toString('hex'),
    sourceVersion,
    targetVersion,
    phase: 'armed',
    completedVersion: null,
  };
  await writeJsonAtomic(path, intent);
  return { path, intent };
}

export async function acknowledgeWindowsUpdateAppReady(
  userDataRoot: string,
  helperExecutable: string,
  currentVersion: string,
  launch: (executable: string, argument: string) => Promise<void>,
): Promise<void> {
  const intentPath = join(resolve(userDataRoot), INTENT_NAME);
  let value: unknown;
  try {
    value = JSON.parse(await readFile(intentPath, 'utf8'));
  } catch {
    return;
  }
  if (
    typeof value !== 'object' ||
    value === null ||
    !('schemaVersion' in value) ||
    value.schemaVersion !== 1 ||
    !('nonce' in value) ||
    typeof value.nonce !== 'string' ||
    !('completedVersion' in value) ||
    value.completedVersion !== currentVersion ||
    !('phase' in value) ||
    !['setup-complete', 'launch-started'].includes(String(value.phase))
  ) {
    return;
  }
  const argument = `--windows-update-app-ready-v1=${Buffer.from(
    JSON.stringify({ intentPath, nonce: value.nonce, version: currentVersion }),
    'utf8',
  ).toString('base64')}`;
  await launch(helperExecutable, argument);
}

export async function clearWindowsUpdateRelaunchIntent(path: string): Promise<void> {
  await rm(path, { force: true });
}

export function wrapWindowsUpdateRelaunchRequest(
  bootstrapArgument: string,
  intentPath: string,
  nonce: string,
): string {
  if (!bootstrapArgument.startsWith('--windows-update-bootstrap-v2=')) {
    throw new Error('Windows update bootstrap request is invalid');
  }
  const payload = Buffer.from(
    JSON.stringify({ request: bootstrapArgument, intentPath, nonce }),
    'utf8',
  ).toString('base64');
  return `--windows-update-bootstrap-v3=${payload}`;
}
