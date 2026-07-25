import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { constants as fsConstants } from 'node:fs';
import { access, readdir, realpath, stat } from 'node:fs/promises';
import { posix, win32, type PlatformPath } from 'node:path';
import { ProviderError } from './errors';
import type { PublicProviderErrorCode } from '../../shared/schemas/providers';

const MAX_PATH_LENGTH = 8_192;
const MAX_PROBE_BYTES = 512 * 1024;
const PROBE_TIMEOUT_MS = 15_000;
const REQUIRED_FLAGS = ['--list-models', '--model', '--thinking'] as const;
const SAFETY_FLAGS = ['--no-tools', '--no-session', '--no-context-files', '--no-approve'] as const;

export type PiDiscoverySource =
  'configured' | 'where' | 'path' | 'appdata-npm' | 'pnpm-home' | 'localappdata-pnpm';

export interface PiCliIdentity {
  readonly canonicalPath: string;
  readonly packageVersion: string;
  readonly discoverySource?: PiDiscoverySource;
  readonly safetyFlags: readonly string[];
  readonly fileIdentity: PiExecutableFileIdentity;
}

export interface PiExecutableFileIdentity {
  readonly dev: string;
  readonly ino: string;
  readonly size: number;
  readonly mtimeMs: number;
}

export interface WindowsSystemTools {
  readonly systemRoot: string;
  readonly system32: string;
  readonly where: string;
  readonly cmd: string;
  readonly taskkill: string;
}

export interface Candidate {
  readonly path: string;
  readonly source: PiDiscoverySource;
}

export interface PiInstallationStatus {
  readonly mode: 'automatic' | 'configured';
  readonly state: 'ready' | 'not-found' | 'invalid' | 'incompatible';
  readonly configuredPath: string | null;
  readonly path: string | null;
  readonly version: string | null;
  readonly source: PiDiscoverySource | null;
  readonly errorCode: PublicProviderErrorCode | null;
}

export async function resolveCanonicalPiCli(
  environment: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
  configuredPath: string | null = null,
  interactiveAppData?: string,
  signal?: AbortSignal,
): Promise<PiCliIdentity> {
  return (
    await discoverPiInstallation(environment, platform, configuredPath, interactiveAppData, signal)
  ).identity;
}

export async function discoverPiInstallation(
  environment: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
  configuredPath: string | null = null,
  interactiveAppData?: string,
  signal?: AbortSignal,
): Promise<{ readonly identity: PiCliIdentity; readonly source: PiDiscoverySource }> {
  throwIfAborted(signal);
  assertNoCaseCollidingEnvironment(environment, platform);
  const configured = configuredPath?.trim() ?? '';
  const paths = targetPath(platform);
  if (configured.length > 0) {
    if (configured.length > MAX_PATH_LENGTH || !paths.isAbsolute(configured))
      throw new PiDiscoveryFailure('PI_CONFIG_INVALID', 'configured', configured);
    try {
      const path = await resolveConfiguredExecutable(configured, environment, platform);
      return {
        identity: await validatePiExecutable(path, environment, platform, signal, 'configured'),
        source: 'configured',
      };
    } catch (error: unknown) {
      if (error instanceof ProviderError)
        throw new PiDiscoveryFailure(error.code, 'configured', configured);
      throw new PiDiscoveryFailure('PI_CONFIG_INVALID', 'configured', configured);
    }
  }

  let incompatible: PiDiscoveryFailure | null = null;
  for (const candidate of await automaticCandidates(environment, platform, interactiveAppData)) {
    throwIfAborted(signal);
    try {
      return {
        identity: await validatePiExecutable(
          candidate.path,
          environment,
          platform,
          signal,
          candidate.source,
        ),
        source: candidate.source,
      };
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code !== 'PI_NOT_FOUND')
        incompatible = new PiDiscoveryFailure(error.code, candidate.source, candidate.path);
    }
  }
  throw incompatible ?? new ProviderError('PI_NOT_FOUND');
}

export async function piInstallationStatus(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  configuredPath: string | null,
  interactiveAppData?: string,
  signal?: AbortSignal,
): Promise<PiInstallationStatus> {
  try {
    const result = await discoverPiInstallation(
      environment,
      platform,
      configuredPath,
      interactiveAppData,
      signal,
    );
    return Object.freeze({
      mode: configuredPath === null ? 'automatic' : 'configured',
      state: 'ready',
      configuredPath,
      path: result.identity.canonicalPath,
      version: result.identity.packageVersion,
      source: result.source,
      errorCode: null,
    });
  } catch (error: unknown) {
    if (signal?.aborted === true) throw new ProviderError('CANCELLED');
    const code = error instanceof ProviderError ? error.code : 'PI_NOT_FOUND';
    return Object.freeze({
      mode: configuredPath === null ? 'automatic' : 'configured',
      state:
        code === 'PI_CONFIG_INVALID'
          ? 'invalid'
          : code === 'PI_INCOMPATIBLE'
            ? 'incompatible'
            : 'not-found',
      configuredPath,
      path: error instanceof PiDiscoveryFailure ? error.candidatePath : null,
      version: null,
      source: error instanceof PiDiscoveryFailure ? error.source : null,
      errorCode: code,
    });
  }
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
    const output = await runBounded(
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
          (value) => value.length > 0 && value.length <= MAX_PATH_LENGTH && win32.isAbsolute(value),
        )
        .map((value) => ({ path: value, source: 'where' as const })),
    );
  } catch {
    return Object.freeze([]);
  }
}

export async function validatePiExecutable(
  input: string,
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  signal?: AbortSignal,
  discoverySource?: PiDiscoverySource,
): Promise<PiCliIdentity> {
  throwIfAborted(signal);
  let canonical: string;
  let fileIdentity: PiExecutableFileIdentity;
  try {
    canonical = await realpath(await caseCorrectPath(input, platform));
    const metadata = await stat(canonical);
    if (!metadata.isFile()) throw new Error('not a file');
    fileIdentity = executableFileIdentity(metadata);
    if (platform !== 'win32') await access(canonical, fsConstants.X_OK);
  } catch {
    throw new ProviderError('PI_NOT_FOUND');
  }
  const extension = targetPath(platform).extname(canonical).toLowerCase();
  if (
    platform === 'win32' &&
    extension !== '' &&
    !['.cmd', '.bat', '.exe', '.com'].includes(extension)
  )
    throw new ProviderError('PI_CONFIG_INVALID');
  try {
    if (platform === 'win32' && /\.(?:cmd|bat)$/iu.test(canonical)) {
      const tools = windowsSystemTools(environment);
      await validateWindowsSystemTool(tools.cmd, tools.system32);
    }
    const version = await runBounded(
      canonical,
      ['--version'],
      environment,
      platform,
      signal,
      PROBE_TIMEOUT_MS,
    );
    const help = await runBounded(
      canonical,
      ['--help'],
      environment,
      platform,
      signal,
      PROBE_TIMEOUT_MS,
    );
    const list = await runBounded(
      canonical,
      ['--list-models'],
      environment,
      platform,
      signal,
      PROBE_TIMEOUT_MS,
    );
    const helpText = `${help.stdout}\n${help.stderr}`;
    const listText = `${list.stdout}\n${list.stderr}`;
    if (
      version.code !== 0 ||
      help.code !== 0 ||
      list.code !== 0 ||
      version.stdout.trim().length === 0 ||
      !hasHelpOption(helpText, '-p') ||
      !REQUIRED_FLAGS.every((flag) => hasHelpOption(helpText, flag)) ||
      /unknown (?:option|argument)[^\r\n]*list-models/iu.test(listText)
    )
      throw new ProviderError('PI_INCOMPATIBLE');
    const versionText = version.stdout.trim().split(/\r?\n/u)[0]?.slice(0, 64) ?? 'compatible';
    return Object.freeze({
      canonicalPath: canonical,
      packageVersion: versionText,
      ...(discoverySource === undefined ? {} : { discoverySource }),
      safetyFlags: Object.freeze(SAFETY_FLAGS.filter((flag) => hasHelpOption(helpText, flag))),
      fileIdentity,
    });
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('PI_LAUNCH_FAILED');
  }
}

export function piSpawnCommand(
  executable: string,
  args: readonly string[],
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): { readonly executable: string; readonly args: readonly string[] } {
  if (platform !== 'win32' || !/\.(?:cmd|bat)$/iu.test(executable)) return { executable, args };
  if (!args.every(isSafePiArgument) || /["%!\r\n\0]/u.test(executable))
    throw new ProviderError('INVALID_CONFIG');
  const command = `""${executable}"${args.length === 0 ? '' : ` ${args.join(' ')}`}"`;
  return {
    executable: windowsSystemTools(environment).cmd,
    args: ['/d', '/s', '/c', command],
  };
}

function isSafePiArgument(value: string): boolean {
  return (
    /^--?[a-z][a-z-]*$/u.test(value) ||
    /^(?:off|minimal|low|medium|high|xhigh)$/u.test(value) ||
    /^[A-Za-z0-9][A-Za-z0-9._:@+/-]{0,511}$/u.test(value)
  );
}

function hasHelpOption(help: string, option: string): boolean {
  const escaped = option.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
  return new RegExp(`(?:^|[\\s,|])${escaped}(?=[\\s,=|<]|$)`, 'mu').test(help);
}

function targetPath(platform: NodeJS.Platform): PlatformPath {
  return platform === 'win32' ? win32 : posix;
}

export function windowsSystemTools(environment: NodeJS.ProcessEnv): WindowsSystemTools {
  assertNoCaseCollidingEnvironment(environment, 'win32');
  const source = environmentValue(environment, 'win32', 'SystemRoot');
  if (source === undefined || !/^[A-Za-z]:[\\/][^\0]*$/u.test(source))
    throw new ProviderError('PI_LAUNCH_FAILED');
  const systemRoot = win32.normalize(source);
  if (!/^[A-Za-z]:\\[^\0]*$/u.test(systemRoot) || systemRoot.includes('..'))
    throw new ProviderError('PI_LAUNCH_FAILED');
  const system32 = win32.join(systemRoot, 'System32');
  const expectedCmd = win32.join(system32, 'cmd.exe');
  const configuredComSpec = environmentValue(environment, 'win32', 'ComSpec');
  if (
    configuredComSpec !== undefined &&
    win32.normalize(configuredComSpec).toLowerCase() !== expectedCmd.toLowerCase()
  )
    throw new ProviderError('PI_LAUNCH_FAILED');
  return Object.freeze({
    systemRoot,
    system32,
    where: win32.join(system32, 'where.exe'),
    cmd: expectedCmd,
    taskkill: win32.join(system32, 'taskkill.exe'),
  });
}

async function validateWindowsSystemTool(tool: string, system32: string): Promise<void> {
  const canonicalRoot = await realpath(system32);
  const canonicalTool = await realpath(tool);
  const metadata = await stat(canonicalTool);
  if (
    !metadata.isFile() ||
    win32.dirname(canonicalTool).toLowerCase() !== canonicalRoot.toLowerCase()
  )
    throw new ProviderError('PI_LAUNCH_FAILED');
}

function executableFileIdentity(
  metadata: Awaited<ReturnType<typeof stat>>,
): PiExecutableFileIdentity {
  return Object.freeze({
    dev: String(metadata.dev),
    ino: String(metadata.ino),
    size: Number(metadata.size),
    mtimeMs: Number(metadata.mtimeMs),
  });
}

async function resolveConfiguredExecutable(
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
    .filter((entry) => entry.length > 0 && entry.length <= MAX_PATH_LENGTH);
}

async function caseCorrectPath(input: string, platform: NodeJS.Platform): Promise<string> {
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

async function runBounded(
  executable: string,
  args: readonly string[],
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  signal: AbortSignal | undefined,
  timeoutMs: number,
): Promise<{ readonly code: number; readonly stdout: string; readonly stderr: string }> {
  const command = piSpawnCommand(executable, args, environment, platform);
  let child: ChildProcessWithoutNullStreams;
  try {
    child = spawn(command.executable, command.args, {
      env: environment,
      shell: false,
      detached: platform !== 'win32',
      windowsHide: true,
      windowsVerbatimArguments: platform === 'win32' && /\.(?:cmd|bat)$/iu.test(executable),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
  } catch {
    throw new ProviderError('PI_LAUNCH_FAILED');
  }
  child.stdin.end();
  return await new Promise((resolveResult, rejectResult) => {
    let settled = false;
    let terminating = false;
    let bytes = 0;
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    const finish = (error: ProviderError | null, code = 1): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      if (error) rejectResult(error);
      else
        resolveResult({
          code,
          stdout: Buffer.concat(stdout).toString('utf8'),
          stderr: Buffer.concat(stderr).toString('utf8'),
        });
    };
    const terminate = (error: ProviderError): void => {
      if (settled || terminating) return;
      terminating = true;
      void terminateProbeProcess(child, platform, environment).then(
        () => finish(error),
        () => finish(new ProviderError('PI_LAUNCH_FAILED')),
      );
    };
    const abort = (): void => terminate(new ProviderError('CANCELLED'));
    const timer = setTimeout(() => terminate(new ProviderError('TIMEOUT')), timeoutMs);
    timer.unref();
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted === true) abort();
    const collect = (target: Buffer[]) => (chunk: Buffer) => {
      bytes += chunk.byteLength;
      if (bytes > MAX_PROBE_BYTES) terminate(new ProviderError('RESPONSE_TOO_LARGE'));
      else target.push(Buffer.from(chunk));
    };
    child.stdout.on('data', collect(stdout));
    child.stderr.on('data', collect(stderr));
    child.once('error', () => terminate(new ProviderError('PI_LAUNCH_FAILED')));
    child.once('close', (code) => {
      if (!terminating) finish(null, code ?? 1);
    });
  });
}

async function terminateProbeProcess(
  child: ChildProcessWithoutNullStreams,
  platform: NodeJS.Platform,
  environment: NodeJS.ProcessEnv,
): Promise<void> {
  const pid = child.pid;
  if (pid === undefined) {
    child.kill('SIGKILL');
    return;
  }
  if (platform !== 'win32') {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {
      child.kill('SIGKILL');
    }
    const deadline = Date.now() + 2_000;
    while (processGroupExists(pid) && Date.now() < deadline)
      await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
    if (processGroupExists(pid)) throw new Error('Pi probe process group did not terminate');
    return;
  }
  const tools = windowsSystemTools(environment);
  await validateWindowsSystemTool(tools.taskkill, tools.system32);
  let killer: ChildProcessWithoutNullStreams;
  try {
    killer = spawn(tools.taskkill, ['/pid', String(pid), '/t', '/f'], {
      shell: false,
      windowsHide: true,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    killer.stdin.end();
  } catch {
    child.kill('SIGKILL');
    throw new Error('Pi probe tree killer did not start');
  }
  const succeeded = await new Promise<boolean>((resolveKill) => {
    let done = false;
    const finish = (value: boolean): void => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolveKill(value);
    };
    const timer = setTimeout(() => {
      killer.kill('SIGKILL');
      finish(false);
    }, 2_000);
    timer.unref();
    killer.once('error', () => finish(false));
    killer.once('close', (code) => finish(code === 0));
  });
  if (!succeeded) {
    child.kill('SIGKILL');
    throw new Error('Pi probe tree killer failed');
  }
  const deadline = Date.now() + 2_000;
  while (processExists(pid) && Date.now() < deadline)
    await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
  if (processExists(pid)) throw new Error('Pi probe root did not terminate');
}

function processGroupExists(pid: number): boolean {
  try {
    process.kill(-pid, 0);
    return true;
  } catch {
    return false;
  }
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function identityKey(identity: PiCliIdentity): string {
  return JSON.stringify([
    identity.canonicalPath,
    identity.packageVersion,
    identity.safetyFlags,
    identity.fileIdentity,
  ]);
}
export async function revalidatePiCliIdentity(
  expected: PiCliIdentity,
  platform: NodeJS.Platform = process.platform,
  signal?: AbortSignal,
): Promise<void> {
  throwIfAborted(signal);
  const current = await realpath(expected.canonicalPath);
  const samePath =
    platform === 'win32'
      ? current.toLowerCase() === expected.canonicalPath.toLowerCase()
      : current === expected.canonicalPath;
  const metadata = await stat(current);
  if (
    !samePath ||
    !metadata.isFile() ||
    JSON.stringify(executableFileIdentity(metadata)) !== JSON.stringify(expected.fileIdentity)
  )
    throw new Error('Pi executable identity changed');
}
function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted === true) throw new ProviderError('CANCELLED');
}
class PiDiscoveryFailure extends ProviderError {
  constructor(
    code: PublicProviderErrorCode,
    readonly source: PiDiscoverySource,
    readonly candidatePath: string,
  ) {
    super(code);
  }
}
function assertNoCaseCollidingEnvironment(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): void {
  if (platform !== 'win32') return;
  const seen = new Set<string>();
  for (const [key, value] of Object.entries(source)) {
    if (value === undefined) continue;
    const normalized = key.toLowerCase();
    if (seen.has(normalized)) throw new ProviderError('PI_LAUNCH_FAILED');
    seen.add(normalized);
  }
}

function environmentValue(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  key: string,
): string | undefined {
  if (platform !== 'win32') return source[key];
  return Object.entries(source).find(
    ([candidate, value]) => value !== undefined && candidate.toLowerCase() === key.toLowerCase(),
  )?.[1];
}
