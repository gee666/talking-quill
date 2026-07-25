import { lstatSync, mkdirSync, realpathSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';

export interface AppPaths {
  readonly root: string;
  readonly settingsFile: string;
  readonly historyDatabase: string;
  readonly credentialsFile: string;
  readonly screenshots: string;
  readonly models: string;
  readonly modelTemporary: string;
  readonly logs: string;
  readonly temporary: string;
  readonly sessionTemporary: string;
}

export function createAppPaths(userDataRoot: string): AppPaths {
  const root = resolve(userDataRoot);
  return Object.freeze({
    root,
    settingsFile: join(root, 'settings.json'),
    historyDatabase: join(root, 'history.db'),
    credentialsFile: join(root, 'credentials.enc'),
    screenshots: join(root, 'screenshots'),
    models: join(root, 'models'),
    modelTemporary: join(root, 'models', '.tmp'),
    logs: join(root, 'logs'),
    temporary: join(root, 'tmp'),
    sessionTemporary: join(root, 'tmp', 'sessions'),
  });
}

/** Validates the user-data entry before any child write; creates only the missing root. */
export function validateAppRootBeforeUse(
  paths: AppPaths,
  allowedBase: string,
  createMissing: boolean,
): boolean {
  const root = resolve(paths.root);
  const base = realpathSync(allowedBase);
  const parent = realpathSync(dirname(root));
  const fromBase = relative(base, parent);
  if (fromBase === '..' || fromBase.startsWith(`..${sep}`) || isAbsolute(fromBase)) {
    throw new Error('Application data parent is outside the validated platform data directory');
  }
  try {
    const metadata = lstatSync(root);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error('Refusing a symbolic-link, junction, or non-directory application data root');
    }
    const canonical = realpathSync(root);
    if (dirname(canonical) !== parent) {
      throw new Error('Application data root changed parent through a link or mount transition');
    }
    return true;
  } catch (error: unknown) {
    if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    if (!createMissing) return false;
    // Deliberately non-recursive: a missing/unvalidated parent must never be created or followed.
    mkdirSync(root, { recursive: false, mode: 0o700 });
    const created = lstatSync(root);
    if (
      created.isSymbolicLink() ||
      !created.isDirectory() ||
      dirname(realpathSync(root)) !== parent
    ) {
      throw new Error('New application data root failed identity validation');
    }
    return true;
  }
}

export function ensureAppDirectories(paths: AppPaths): void {
  const root = lstatSync(paths.root);
  if (root.isSymbolicLink() || !root.isDirectory()) {
    throw new Error('Application data root is not a validated directory');
  }
  for (const directory of [
    paths.screenshots,
    paths.models,
    paths.modelTemporary,
    paths.logs,
    paths.temporary,
    paths.sessionTemporary,
  ]) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
