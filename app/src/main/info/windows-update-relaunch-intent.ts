import { randomBytes } from 'node:crypto';
import { rm } from 'node:fs/promises';
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

const RELAUNCH_GENERATION_PREFIX = '--windows-update-relaunch-generation-v1=';

export function readWindowsUpdateRelaunchGeneration(commandLine: readonly string[]): string | null {
  const matches = commandLine
    .filter((argument) => argument.startsWith(RELAUNCH_GENERATION_PREFIX))
    .map((argument) => argument.slice(RELAUNCH_GENERATION_PREFIX.length));
  return matches.length === 1 && /^[0-9a-f]{32}$/.test(matches[0] ?? '')
    ? (matches[0] ?? null)
    : null;
}

export async function acknowledgeWindowsUpdateAppReady(
  helperExecutable: string,
  currentVersion: string,
  generation: string,
  launch: (executable: string, argument: string) => Promise<void>,
): Promise<void> {
  if (!/^[0-9a-f]{32}$/.test(generation)) return;
  const argument = `--windows-update-app-ready-v1=${Buffer.from(
    JSON.stringify({ generation, version: currentVersion }),
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
