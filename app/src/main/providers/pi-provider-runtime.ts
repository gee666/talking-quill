import { ProviderError } from './errors';
import { resolveCanonicalPiCli } from './pi-discovery';
import { revalidatePiCliIdentity, type PiCliIdentity } from './pi-executable';
import { PiOperationScheduler } from './pi-operation-scheduler';
import { runPiInvocation, type SpawnPi } from './pi-process-runtime';
import { MAX_PI_STDOUT_BYTES } from './pi-models';
import type { PiProviderOptions } from './pi-provider-options';

export const DEFAULT_PI_TIMEOUT_MS = 120_000;
const PI_TERMINATION_RESERVE_MS = 5_000;
export const PI_MIN_OPERATION_TIMEOUT_MS = PI_TERMINATION_RESERVE_MS + 500;
const MAX_STDOUT_BYTES = MAX_PI_STDOUT_BYTES;
const MAX_STDERR_BYTES = 16 * 1024;

/** Owns CLI identity and serialization; identity loss invalidates the provider's model cache. */
export class PiProviderRuntime {
  readonly spawnPi: SpawnPi | undefined;
  readonly environment: NodeJS.ProcessEnv;
  readonly platform: NodeJS.Platform;
  readonly workingDirectory: string;
  readonly configuredPath: () => string | null;
  readonly #resolveCli: (
    configuredPath: string | null,
    signal?: AbortSignal,
  ) => Promise<PiCliIdentity>;
  readonly revalidateCli: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
  readonly scheduler = new PiOperationScheduler();
  #identity: { readonly configuredPath: string | null; readonly value: PiCliIdentity } | null =
    null;
  readonly #onIdentityInvalidated: () => void;

  constructor(options: PiProviderOptions, onIdentityInvalidated: () => void) {
    this.#onIdentityInvalidated = onIdentityInvalidated;
    this.spawnPi = options.spawnPi;
    this.platform = options.platform ?? process.platform;
    this.environment = withInteractiveHome(
      options.environment ?? process.env,
      this.platform,
      options.interactiveHome,
    );
    this.workingDirectory = options.workingDirectory ?? process.cwd();
    this.configuredPath = options.configuredPath ?? (() => null);
    const customResolveCli = options.resolveCli;
    this.#resolveCli =
      customResolveCli === undefined
        ? (configuredPath, signal) =>
            resolveCanonicalPiCli(
              this.environment,
              this.platform,
              configuredPath,
              options.interactiveAppData,
              signal,
            )
        : (_configuredPath, signal) => customResolveCli(signal);
    this.revalidateCli =
      options.revalidateCli ??
      (options.resolveCli === undefined
        ? (identity, signal) => revalidatePiCliIdentity(identity, this.platform, signal)
        : () => Promise.resolve());
  }

  invalidateIdentity(): void {
    this.#identity = null;
    this.#onIdentityInvalidated();
  }

  async resolvePreparedIdentity(
    initial: PiCliIdentity,
    configuredPath: string | null,
    signal: AbortSignal,
  ): Promise<PiCliIdentity> {
    try {
      await waitForPiAbort(this.revalidateCli(initial, signal), signal);
      return initial;
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.invalidateIdentity();
      return await this.resolveIdentity(signal, configuredPath, true);
    }
  }

  async resolveIdentity(
    signal: AbortSignal,
    configuredPath = this.configuredPath(),
    operationHeld = false,
  ): Promise<PiCliIdentity> {
    const cached = await this.#revalidatedCachedIdentity(configuredPath, signal);
    if (cached !== null) return cached;
    const permit = operationHeld ? null : await this.scheduler.acquireForeground(signal);
    try {
      // Another queued resolver may have populated the same frozen configured path.
      const queuedCache = await this.#revalidatedCachedIdentity(configuredPath, signal);
      if (queuedCache !== null) return queuedCache;
      try {
        const value = await waitForPiAbort(this.#resolveCli(configuredPath, signal), signal);
        this.#identity = Object.freeze({ configuredPath, value });
        return value;
      } catch (error: unknown) {
        if (error instanceof ProviderError) throw error;
        throw new ProviderError('PI_NOT_FOUND');
      }
    } finally {
      permit?.release();
    }
  }

  async #revalidatedCachedIdentity(
    configuredPath: string | null,
    signal: AbortSignal,
  ): Promise<PiCliIdentity | null> {
    if (this.#identity?.configuredPath !== configuredPath) return null;
    try {
      await waitForPiAbort(this.revalidateCli(this.#identity.value, signal), signal);
      return this.#identity.value;
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.invalidateIdentity();
      return null;
    }
  }

  async runResolved(
    resolvedIdentity: PiCliIdentity,
    args: readonly string[],
    input: string | null,
    signal: AbortSignal,
    timeoutMs: number,
  ): Promise<string> {
    if (signal.aborted) throw new ProviderError('CANCELLED');
    const permit = await this.scheduler.acquireForeground(signal);
    try {
      let identity = resolvedIdentity;
      try {
        await waitForPiAbort(this.revalidateCli(identity, signal), signal);
      } catch (error: unknown) {
        if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
        this.invalidateIdentity();
        identity = await this.resolveIdentity(signal, this.configuredPath(), true);
      }
      const result = await runPiInvocation(identity.canonicalPath, args, input, signal, timeoutMs, {
        spawnPi: this.spawnPi,
        environment: this.environment,
        platform: this.platform,
        workingDirectory: this.workingDirectory,
        maxStdoutBytes: MAX_STDOUT_BYTES,
        maxStderrBytes: MAX_STDERR_BYTES,
      });
      if (result.code === 0) return result.stdout;
      throw classifyPiFailure(result.stderr);
    } finally {
      permit.release();
    }
  }
}

export function classifyPiFailure(stderr: string): ProviderError {
  const value = stderr.toLowerCase();
  if (/no api key|authentication|unauthori[sz]ed|log in|\/login/u.test(value))
    return new ProviderError('AUTHENTICATION_FAILED');
  if (/rate limit|too many requests/u.test(value)) return new ProviderError('RATE_LIMITED');
  if (/model[^\r\n]*(?:not found|not available|unknown)/u.test(value))
    return new ProviderError('MODEL_NOT_FOUND');
  if (/unknown option|invalid (?:option|argument)|usage:/u.test(value))
    return new ProviderError('PI_LAUNCH_FAILED');
  return new ProviderError('REMOTE_FAILURE');
}
function withInteractiveHome(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  home: string | undefined,
): NodeJS.ProcessEnv {
  if (platform !== 'win32' || home === undefined) return source;
  return Object.fromEntries([
    ...Object.entries(source).filter(([key]) => key.toLowerCase() !== 'userprofile'),
    ['USERPROFILE', home],
  ]);
}
export function waitForPiAbort<Result>(
  operation: Promise<Result>,
  signal: AbortSignal,
): Promise<Result> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  return new Promise((resolveOperation, rejectOperation) => {
    const abort = (): void => rejectOperation(new ProviderError('CANCELLED'));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (result) => {
        signal.removeEventListener('abort', abort);
        resolveOperation(result);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        rejectOperation(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
