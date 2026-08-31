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
  };
  await writeJsonAtomic(path, intent);
  return { path, intent };
}

export async function clearStaleWindowsUpdateRelaunchIntent(userDataRoot: string): Promise<void> {
  await clearWindowsUpdateRelaunchIntent(join(resolve(userDataRoot), INTENT_NAME));
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
