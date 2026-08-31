import { resolve } from 'node:path';

/** Resolve the only per-user tree that the packaged uninstall reset may remove. */
export function resolveSignedInWindowsUserDataTarget(roamingAppData: string): string {
  if (roamingAppData.length === 0 || roamingAppData.includes('\0')) {
    throw new Error('Windows roaming AppData root is unavailable');
  }
  const root = resolve(roamingAppData);
  const target = resolve(root, 'Talking Quill');
  if (target === root || !target.startsWith(`${root}\\`)) {
    throw new Error('Could not resolve the signed-in Windows user data target');
  }
  return target;
}
