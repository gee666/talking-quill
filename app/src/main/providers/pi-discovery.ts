import { posix, win32 } from 'node:path';
import type { PublicProviderErrorCode } from '../../shared/schemas/providers';
import {
  MAX_PI_PATH_LENGTH,
  automaticCandidates,
  resolveConfiguredExecutable,
  type PiDiscoverySource,
} from './pi-candidates';
import { ProviderError } from './errors';
import { validatePiExecutable, type PiCliIdentity } from './pi-executable';
import { assertNoCaseCollidingEnvironment } from './pi-process-runtime';

export type { Candidate, PiDiscoverySource } from './pi-candidates';
export { automaticCandidates, windowsWhereCandidates } from './pi-candidates';
export type { PiCliIdentity, PiExecutableFileIdentity } from './pi-executable';
export { identityKey, revalidatePiCliIdentity, validatePiExecutable } from './pi-executable';
export type { WindowsSystemTools } from './pi-process-runtime';
export { piSpawnCommand, windowsSystemTools } from './pi-process-runtime';

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
  const paths = platform === 'win32' ? win32 : posix;
  if (configured.length > 0) {
    if (configured.length > MAX_PI_PATH_LENGTH || !paths.isAbsolute(configured))
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
