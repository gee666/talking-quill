import {
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
  ProviderConfigSchema,
  type Destination,
  type ModelInfo,
  type ProviderCompletionRequest,
  type ProviderConfig,
  type ProviderValidationResult,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import type { EgressObserver } from '../security/egress-audit';
import { ProviderError } from './errors';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import { resolveCanonicalPiCli } from './pi-discovery';
import { identityKey, revalidatePiCliIdentity, type PiCliIdentity } from './pi-executable';
import { runPiInvocation, type SpawnPi } from './pi-process-runtime';
export { resolveCanonicalPiCli } from './pi-discovery';
export type { PiCliIdentity } from './pi-executable';
export { terminateProcessTree } from './pi-process-runtime';
export type { PiTreeTerminationOptions, SpawnPi } from './pi-process-runtime';

const MODEL_CACHE_TTL_MS = 5 * 60_000;
const MAX_STDOUT_BYTES = 2 * 1024 * 1024;
const MAX_STDERR_BYTES = 16 * 1024;
const MAX_MODELS = 5_000;
const DEFAULT_TIMEOUT_MS = 120_000;
const PI_TERMINATION_RESERVE_MS = 5_000;
const PI_MIN_OPERATION_TIMEOUT_MS = PI_TERMINATION_RESERVE_MS + 500;
const CONNECTION_TEST_PROMPT = 'Reply with exactly: TALKING_QUILL_CONNECTION_OK';
const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._:@+-]{0,127}\/[A-Za-z0-9][A-Za-z0-9._:@+-]{0,383}$/u;

export interface PiProviderOptions {
  readonly spawnPi?: SpawnPi;
  readonly environment?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
  readonly workingDirectory?: string;
  readonly now?: () => number;
  readonly observeEgress?: EgressObserver;
  readonly configuredPath?: () => string | null;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
  readonly resolveCli?: (signal?: AbortSignal) => Promise<PiCliIdentity>;
  readonly revalidateCli?: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
}

type PiConfig = ProviderConfig & {
  readonly providerId: 'pi';
  readonly modelId: string;
  readonly thinking: NonNullable<ProviderConfig['thinking']>;
};
interface ModelCatalog {
  readonly key: string;
  readonly models: readonly ModelInfo[];
}
interface ModelCache extends ModelCatalog {
  readonly expiresAt: number;
}

export class PiProvider implements SmartProvider {
  readonly id = 'pi' as const;
  readonly credentialPolicy = 'none' as const;
  readonly #spawnPi: SpawnPi | undefined;
  readonly #environment: NodeJS.ProcessEnv;
  readonly #platform: NodeJS.Platform;
  readonly #workingDirectory: string;
  readonly #now: () => number;
  readonly #observeEgress: EgressObserver;
  readonly #configuredPath: () => string | null;
  readonly #resolveCli: (signal?: AbortSignal) => Promise<PiCliIdentity>;
  readonly #revalidateCli: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
  #identity: { readonly configuredPath: string | null; readonly value: PiCliIdentity } | null =
    null;
  #models: ModelCache | null = null;
  #operationQueue: Promise<void> = Promise.resolve();

  constructor(options: PiProviderOptions = {}) {
    this.#spawnPi = options.spawnPi;
    this.#platform = options.platform ?? process.platform;
    this.#environment = withInteractiveHome(
      options.environment ?? process.env,
      this.#platform,
      options.interactiveHome,
    );
    this.#workingDirectory = options.workingDirectory ?? process.cwd();
    this.#now = options.now ?? Date.now;
    this.#observeEgress = options.observeEgress ?? (() => undefined);
    this.#configuredPath = options.configuredPath ?? (() => null);
    this.#resolveCli =
      options.resolveCli ??
      ((signal) =>
        resolveCanonicalPiCli(
          this.#environment,
          this.#platform,
          this.#configuredPath(),
          options.interactiveAppData,
          signal,
        ));
    this.#revalidateCli =
      options.revalidateCli ??
      (options.resolveCli === undefined
        ? (identity, signal) => revalidatePiCliIdentity(identity, this.#platform, signal)
        : () => Promise.resolve());
  }

  credentialBinding(config: ProviderConfig): string {
    this.#baseConfig(config);
    return 'pi:user-cli';
  }

  async validate(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ProviderValidationResult> {
    const config = this.#runtimeConfig(invocation.config);
    const identity = await this.#resolveIdentity(signal);
    const models = parsePiModels(
      await this.#runResolved(identity, ['--list-models'], null, signal, DEFAULT_TIMEOUT_MS),
    );
    if (models.length > 0 && !models.some(({ id }) => id === config.modelId))
      throw new ProviderError('MODEL_NOT_FOUND');
    this.#observeEgress('provider');
    const output = await this.#runResolved(
      identity,
      ['-p', '--model', config.modelId, '--thinking', config.thinking, ...identity.safetyFlags],
      CONNECTION_TEST_PROMPT,
      signal,
      Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
    );
    if (output.trim().length === 0) throw new ProviderError('INVALID_RESPONSE');
    return Object.freeze({ ok: true, destination: 'cloud', modelCount: models.length });
  }

  async listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    this.#baseConfig(invocation.config);
    const identity = await this.#resolveIdentity(signal);
    const key = identityKey(identity);
    if (
      invocation.refreshModels !== true &&
      this.#models?.key === key &&
      this.#models.expiresAt > this.#now()
    )
      return this.#models.models;
    this.#observeEgress('provider');
    const models = parsePiModels(
      await this.#runResolved(identity, ['--list-models'], null, signal, DEFAULT_TIMEOUT_MS),
    );
    this.#models = { key, models, expiresAt: this.#now() + MODEL_CACHE_TTL_MS };
    return models;
  }

  capabilities(): VisionCapability {
    return 'unsupported';
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    requestInput: ProviderCompletionRequest,
    signal: AbortSignal,
  ): Promise<string> {
    const config = this.#runtimeConfig(invocation.config);
    const request = ProviderCompletionRequestSchema.parse(requestInput);
    if (request.image !== undefined) throw new ProviderError('INVALID_CONFIG');
    const modelId = request.modelId ?? config.modelId;
    assertModelId(modelId);
    this.#observeEgress('provider');
    try {
      const identity = await this.#resolveIdentity(signal);
      const output = await this.#runResolved(
        identity,
        ['-p', '--model', modelId, '--thinking', config.thinking, ...identity.safetyFlags],
        request.input,
        signal,
        Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
      );
      if (output.length > MAX_NATIVE_OUTPUT_CHARACTERS || output.trim().length === 0)
        throw new ProviderError('INVALID_RESPONSE');
      return output.trim();
    } catch (error: unknown) {
      this.#models = null;
      throw error;
    }
  }

  classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination> {
    this.#baseConfig(invocation.config);
    return signal.aborted
      ? Promise.reject(new ProviderError('CANCELLED'))
      : Promise.resolve('cloud');
  }

  #baseConfig(input: ProviderConfig): ProviderConfig & { readonly providerId: 'pi' } {
    try {
      const config = ProviderConfigSchema.parse(input);
      if (config.providerId !== 'pi') throw new ProviderError('INVALID_CONFIG');
      return config as ProviderConfig & { readonly providerId: 'pi' };
    } catch (error: unknown) {
      if (error instanceof ProviderError) throw error;
      throw new ProviderError('INVALID_CONFIG');
    }
  }

  #runtimeConfig(input: ProviderConfig): PiConfig {
    const config = this.#baseConfig(input);
    if (config.modelId == null || config.thinking === undefined)
      throw new ProviderError('INVALID_CONFIG');
    assertModelId(config.modelId);
    return config as PiConfig;
  }

  async #resolveIdentity(signal: AbortSignal): Promise<PiCliIdentity> {
    const configuredPath = this.#configuredPath();
    if (this.#identity?.configuredPath === configuredPath) {
      try {
        await waitForAbort(this.#revalidateCli(this.#identity.value, signal), signal);
        return this.#identity.value;
      } catch (error: unknown) {
        if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
        this.#identity = null;
        this.#models = null;
      }
    }
    try {
      const value = await waitForAbort(this.#resolveCli(signal), signal);
      this.#identity = Object.freeze({ configuredPath, value });
      return value;
    } catch (error: unknown) {
      if (error instanceof ProviderError) throw error;
      throw new ProviderError('PI_NOT_FOUND');
    }
  }

  async #runResolved(
    resolvedIdentity: PiCliIdentity,
    args: readonly string[],
    input: string | null,
    signal: AbortSignal,
    timeoutMs: number,
  ): Promise<string> {
    if (signal.aborted) throw new ProviderError('CANCELLED');
    const release = await this.#acquireOperation(signal);
    try {
      let identity = resolvedIdentity;
      try {
        await waitForAbort(this.#revalidateCli(identity, signal), signal);
      } catch (error: unknown) {
        if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
        this.#identity = null;
        this.#models = null;
        identity = await this.#resolveIdentity(signal);
      }
      const result = await runPiInvocation(identity.canonicalPath, args, input, signal, timeoutMs, {
        spawnPi: this.#spawnPi,
        environment: this.#environment,
        platform: this.#platform,
        workingDirectory: this.#workingDirectory,
        maxStdoutBytes: MAX_STDOUT_BYTES,
        maxStderrBytes: MAX_STDERR_BYTES,
      });
      if (result.code === 0) return result.stdout;
      throw classifyPiFailure(result.stderr);
    } finally {
      release();
    }
  }

  async #acquireOperation(signal: AbortSignal): Promise<() => void> {
    const previous = this.#operationQueue;
    let release = (): void => undefined;
    const current = new Promise<void>((resolveTurn) => {
      release = resolveTurn;
    });
    this.#operationQueue = previous.catch(() => undefined).then(() => current);
    try {
      await waitForAbort(previous, signal);
      return release;
    } catch (error: unknown) {
      release();
      throw error;
    }
  }
}

export function parsePiModels(output: string): readonly ModelInfo[] {
  if (Buffer.byteLength(output, 'utf8') > MAX_STDOUT_BYTES)
    throw new ProviderError('RESPONSE_TOO_LARGE');
  const lines = output
    .replace(/^\uFEFF/u, '')
    .replace(new RegExp(`${String.fromCharCode(27)}\\[[0-?]*[ -/]*[@-~]`, 'gu'), '')
    .replace(/\r/g, '')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0 || /^no models/iu.test(lines[0] ?? '')) return Object.freeze([]);
  const headerIndex = lines.findIndex((line) => /provider/iu.test(line) && /model/iu.test(line));
  if (headerIndex < 0) return Object.freeze([]);
  const header = (lines[headerIndex] ?? '').toLowerCase().split(/\s{2,}|\t+|\s+/u);
  const providerIndex = header.indexOf('provider');
  const modelIndex = header.indexOf('model');
  const contextIndex = header.findIndex((value) => value.startsWith('context'));
  const imagesIndex = header.findIndex((value) => value.startsWith('image'));
  if (providerIndex < 0 || modelIndex < 0) return Object.freeze([]);
  const models: ModelInfo[] = [];
  const seen = new Set<string>();
  for (const line of lines.slice(headerIndex + 1)) {
    const columns = line.split(/\s{2,}|\t+/u).filter(Boolean);
    const fallback =
      columns.length < Math.max(providerIndex, modelIndex) + 1 ? line.split(/\s+/u) : columns;
    const provider = fallback[providerIndex];
    const model = fallback[modelIndex];
    if (provider === undefined || model === undefined) continue;
    const id = model.includes('/') ? model : `${provider}/${model}`;
    if (!MODEL_ID.test(id) || seen.has(id)) continue;
    let contextWindow: number | null = null;
    if (contextIndex >= 0 && fallback[contextIndex] !== undefined)
      contextWindow = parseCompactTokens(fallback[contextIndex] ?? '');
    const vision =
      imagesIndex >= 0 && /^(?:yes|true|supported)$/iu.test(fallback[imagesIndex] ?? '')
        ? 'supported'
        : 'unsupported';
    const parsed = ModelInfoSchema.safeParse({ id, name: id, contextWindow, vision });
    if (parsed.success) {
      seen.add(id);
      models.push(parsed.data);
    }
    if (models.length >= MAX_MODELS) break;
  }
  return Object.freeze(models.sort((left, right) => left.id.localeCompare(right.id)));
}

function parseCompactTokens(value: string): number | null {
  const match = /^(\d+(?:\.\d+)?)([kKmM])?$/u.exec(value);
  if (match?.[1] === undefined) return null;
  const multiplier =
    match[2]?.toLowerCase() === 'm' ? 1_000_000 : match[2]?.toLowerCase() === 'k' ? 1_000 : 1;
  const result = Number(match[1]) * multiplier;
  return Number.isSafeInteger(result) && result > 0 && result <= 2_000_000 ? result : null;
}
function assertModelId(modelId: string): void {
  if (!MODEL_ID.test(modelId)) throw new ProviderError('INVALID_CONFIG');
}
function classifyPiFailure(stderr: string): ProviderError {
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
function waitForAbort<Result>(operation: Promise<Result>, signal: AbortSignal): Promise<Result> {
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
