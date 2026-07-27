import { constants as fsConstants } from 'node:fs';
import { access, realpath, stat } from 'node:fs/promises';
import { posix, win32, type PlatformPath } from 'node:path';
import { caseCorrectPath, type PiDiscoverySource } from './pi-candidates';
import { ProviderError } from './errors';
import {
  runBoundedPiProcess,
  validateWindowsSystemTool,
  windowsSystemTools,
} from './pi-process-runtime';

const PROBE_TIMEOUT_MS = 15_000;
const REQUIRED_FLAGS = ['--list-models', '--model', '--thinking'] as const;
const SAFETY_FLAGS = [
  '--no-tools',
  '--no-session',
  '--no-context-files',
  '--no-approve',
  '--no-skills',
  '--no-prompt-templates',
  '--no-themes',
  '--offline',
] as const;

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
    const version = await runBoundedPiProcess(
      canonical,
      ['--version'],
      environment,
      platform,
      signal,
      PROBE_TIMEOUT_MS,
    );
    const help = await runBoundedPiProcess(
      canonical,
      ['--help', ...SAFETY_FLAGS],
      environment,
      platform,
      signal,
      PROBE_TIMEOUT_MS,
    );
    const helpText = `${help.stdout}\n${help.stderr}`;
    if (
      version.code !== 0 ||
      help.code !== 0 ||
      version.stdout.trim().length === 0 ||
      !hasHelpOption(helpText, '-p') ||
      ![...REQUIRED_FLAGS, ...SAFETY_FLAGS].every((flag) => hasHelpOption(helpText, flag))
    )
      throw new ProviderError('PI_INCOMPATIBLE');
    const versionText = version.stdout.trim().split(/\r?\n/u)[0]?.slice(0, 64) ?? 'compatible';
    return Object.freeze({
      canonicalPath: canonical,
      packageVersion: versionText,
      ...(discoverySource === undefined ? {} : { discoverySource }),
      safetyFlags: Object.freeze([...SAFETY_FLAGS]),
      fileIdentity,
    });
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('PI_LAUNCH_FAILED');
  }
}

function hasHelpOption(help: string, option: string): boolean {
  const escaped = option.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
  return new RegExp(`(?:^|[\\s,|])${escaped}(?=[\\s,=|<]|$)`, 'mu').test(help);
}

function targetPath(platform: NodeJS.Platform): PlatformPath {
  return platform === 'win32' ? win32 : posix;
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
