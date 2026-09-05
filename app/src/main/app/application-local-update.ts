import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { type DownloadedApplicationUpdate } from '../info/macos-owner-update-coordinator';
import { parseUnsignedUpdateIdentity } from '../info/unsigned-update-identity';

export function localMacosUpdate(archive: string): DownloadedApplicationUpdate {
  if (process.arch !== 'x64' && process.arch !== 'arm64') {
    throw new Error('The current macOS architecture cannot validate local updater identity');
  }
  const identityPath = join(dirname(archive), `release-identity-mac-${process.arch}.json`);
  let value: unknown;
  try {
    value = JSON.parse(readFileSync(identityPath, 'utf8')) as unknown;
  } catch (error: unknown) {
    throw new Error('The local macOS update identity sidecar is missing or invalid', {
      cause: error,
    });
  }
  const version = (value as { readonly version?: unknown }).version;
  if (typeof version !== 'string') {
    throw new Error('The local macOS update identity has no release version');
  }
  return {
    files: [archive],
    identity: parseUnsignedUpdateIdentity(value, 'darwin', process.arch, version),
  };
}
