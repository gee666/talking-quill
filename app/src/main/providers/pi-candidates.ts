import { readdir, stat } from 'node:fs/promises';
import { posix, win32, type PlatformPath } from 'node:path';
import {
  environmentValue,
  runBoundedPiProcess,
  validateWindowsSystemTool,
  windowsSystemTools,
} from './pi-process-runtime';

export const MAX_PI_PATH_LENGTH = 8_192;

export type PiDiscoverySource =
  'configured' | 'where' | 'path' | 'appdata-npm' | 'pnpm-home' | 'localappdata-pnpm';

export interface Candidate {
  readonly path: string;
  readonly source: PiDiscoverySource;
}

export async function automaticCandidates(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  interactiveAppData?: string,
): Promise<readonly Candidate[]> {
  const candidates: Candidate[] = [];
  if (platform === 'win32') candidates.push(...(await windowsWhereCandidates(environment)));
  const names = executableNames(environment, platform);
  const paths = targetPath(platform);
  const path = environmentValue(environment, platform, 'PATH');
  for (const directory of splitPath(path, platform))
    for (const name of names)
      candidates.push({ path: paths.join(directory, name), source: 'path' });
  if (platform === 'win32') {
    const profile = environmentValue(environment, platform, 'USERPROFILE');
    const appData = environmentValue(environment, platform, 'APPDATA');
    const local =
      environmentValue(environment, platform, 'LOCALAPPDATA') ??
      (profile === undefined ? undefined : paths.join(profile, 'AppData', 'Local'));
    const pnpm = environmentValue(environment, platform, 'PNPM_HOME');
    for (const home of [
      interactiveAppData,
      appData,
      profile === undefined ? undefined : paths.join(profile, 'AppData', 'Roaming'),
    ])
      if (home !== undefined && paths.isAbsolute(home))
        for (const name of names)
          candidates.push({ path: paths.join(home, 'npm', name), source: 'appdata-npm' });
    if (pnpm !== undefined && paths.isAbsolute(pnpm))
      for (const name of names)
        candidates.push({ path: paths.join(pnpm, name), source: 'pnpm-home' });
    if (local !== undefined && paths.isAbsolute(local))
      for (const name of names)
        candidates.push({ path: paths.join(local, 'pnpm', name), source: 'localappdata-pnpm' });
  }
  const seen = new Set<string>();
  return Object.freeze(
    candidates.filter(({ path }) => {
      const key = platform === 'win32' ? path.toLowerCase() : path;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    }),
  );
}

export async function windowsWhereCandidates(
  environment: NodeJS.ProcessEnv,
): Promise<readonly Candidate[]> {
  const path = environmentValue(environment, 'win32', 'PATH') ?? '';
  if (path.length > 32_767 || path.includes('\0')) return Object.freeze([]);
  try {
    const tools = windowsSystemTools(environment);
    await validateWindowsSystemTool(tools.where, tools.system32);
    const output = await runBoundedPiProcess(
      tools.where,
      ['pi'],
      { ...environment, PATH: path },
      'win32',
      undefined,
      2_000,
    );
    return Object.freeze(
      output.stdout
        .split(/\r?\n/u)
        .map((value) => value.trim())
        .filter(
          (value) =>
            value.length > 0 && value.length <= MAX_PI_PATH_LENGTH && win32.isAbsolute(value),
        )
        .map((value) => ({ path: value, source: 'where' as const })),
    );
  } catch {
    return Object.freeze([]);
  }
}

export async function resolveConfiguredExecutable(
  input: string,
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): Promise<string> {
  const corrected = await caseCorrectPath(input, platform);
  const metadata = await stat(corrected);
  if (!metadata.isDirectory()) return corrected;
  const paths = targetPath(platform);
  for (const name of executableNames(environment, platform)) {
    try {
      const candidate = await caseCorrectPath(paths.join(corrected, name), platform);
      if ((await stat(candidate)).isFile()) return candidate;
    } catch {
      /* next candidate */
    }
  }
  throw new Error('directory does not contain pi');
}

export async function caseCorrectPath(input: string, platform: NodeJS.Platform): Promise<string> {
  if (platform !== 'win32') return input;
  try {
    await stat(input);
    return input;
  } catch {
    /* emulate Windows case folding for fixtures */
  }
  const paths = targetPath(platform);
  const parent = paths.dirname(input);
  if (parent === input) return input;
  const correctedParent = await caseCorrectPath(parent, platform);
  const wanted = paths.basename(input).toLowerCase();
  const match = (await readdir(correctedParent)).find((entry) => entry.toLowerCase() === wanted);
  return match === undefined ? input : paths.join(correctedParent, match);
}

function executableNames(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): readonly string[] {
  if (platform !== 'win32') return ['pi'];
  const extensions = (environmentValue(environment, platform, 'PATHEXT') ?? '.COM;.EXE;.BAT;.CMD')
    .split(';')
    .map((value) => value.trim().toLowerCase())
    .filter((value) => /^\.[a-z0-9]+$/u.test(value));
  return Object.freeze([...new Set([...extensions.map((extension) => `pi${extension}`), 'pi'])]);
}

function splitPath(value: string | undefined, platform: NodeJS.Platform): readonly string[] {
  if (value === undefined) return [];
  return value
    .split(platform === 'win32' ? ';' : ':')
    .map((entry) => entry.trim().replace(/^"|"$/gu, ''))
    .filter((entry) => entry.length > 0 && entry.length <= MAX_PI_PATH_LENGTH);
}

function targetPath(platform: NodeJS.Platform): PlatformPath {
  return platform === 'win32' ? win32 : posix;
}
