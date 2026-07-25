import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { IdentityBoundRemovalRequest } from './data-lifecycle-service';

const execFileAsync = promisify(execFile);
const NATIVE_REMOVAL_TIMEOUT_MS = 30_000;

export type NativeRemovalPlatform = 'win32' | 'darwin';

/** Invokes the bundled native boundary; the helper never receives an unverified path alone. */
export function createNativeOwnedTreeRemoval(
  executablePath: string,
  platform: NodeJS.Platform = process.platform,
): (request: IdentityBoundRemovalRequest) => Promise<void> {
  if (platform !== 'win32' && platform !== 'darwin') {
    return () =>
      Promise.reject(new Error('Identity-bound recursive reset deletion is unavailable'));
  }
  return async (request) => {
    if (!/^\d+:\d+$/u.test(request.expectedFileIdentity)) {
      throw new Error('Identity-bound reset request has an invalid file identity');
    }
    await execFileAsync(
      executablePath,
      ['--remove-owned-tree', request.path, request.expectedFileIdentity],
      {
        windowsHide: true,
        timeout: NATIVE_REMOVAL_TIMEOUT_MS,
        maxBuffer: 16 * 1024,
      },
    );
  };
}
