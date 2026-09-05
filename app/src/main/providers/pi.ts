import {
  ProviderCompletionRequestSchema,
  type Destination,
  type ModelInfo,
  type ProviderCompletionRequest,
  type ProviderConfig,
  type ProviderValidationResult,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type {
  PreparedProviderCompletion,
  ProviderInvocationConfig,
  SmartProvider,
} from './contracts';
import type { EgressObserver } from '../security/egress-audit';
import { ProviderError } from './errors';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import { identityKey } from './pi-executable';
import { PiExtensionResolver } from './pi-extension-resolver';
import {
  assertModelId,
  basePiConfig,
  runtimePiConfig,
  freezePiConfig,
  parsePiModels,
} from './pi-models';
import { PiPreparedCompletionFactory } from './pi-prepared-completion';
import type { PiProviderOptions } from './pi-provider-options';
import {
  PiProviderRuntime,
  DEFAULT_PI_TIMEOUT_MS as DEFAULT_TIMEOUT_MS,
  PI_MIN_OPERATION_TIMEOUT_MS,
} from './pi-provider-runtime';
export { resolveCanonicalPiCli } from './pi-discovery';
export type { PiCliIdentity } from './pi-executable';
export { terminateProcessTree } from './pi-process-runtime';
export type { PiTreeTerminationOptions, SpawnPi } from './pi-process-runtime';
export type { PiProviderOptions } from './pi-provider-options';
export { parsePiModels } from './pi-models';
export { PI_RPC_READY_TTL_MS } from './pi-prepared-completion';

const MODEL_CACHE_TTL_MS = 5 * 60_000;
const CONNECTION_TEST_PROMPT = 'Reply with exactly: TALKING_QUILL_CONNECTION_OK';

interface ModelCache {
  readonly key: string;
  readonly models: readonly ModelInfo[];
  readonly expiresAt: number;
}

export class PiProvider implements SmartProvider {
  readonly id = 'pi' as const;
  readonly credentialPolicy = 'none' as const;
  readonly #now: () => number;
  readonly #observeEgress: EgressObserver;
  readonly #runtime: PiProviderRuntime;
  readonly #extensions: PiExtensionResolver;
  readonly #prepared: PiPreparedCompletionFactory;
  #models: ModelCache | null = null;

  constructor(options: PiProviderOptions = {}) {
    this.#now = options.now ?? Date.now;
    this.#observeEgress = options.observeEgress ?? (() => undefined);
    this.#runtime = new PiProviderRuntime(options, () => {
      this.#models = null;
    });
    this.#extensions = new PiExtensionResolver(options, this.#runtime);
    this.#prepared = new PiPreparedCompletionFactory(options, this.#runtime, this.#extensions);
  }

  credentialBinding(config: ProviderConfig): string {
    basePiConfig(config);
    return 'pi:user-cli';
  }

  async validate(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ProviderValidationResult> {
    const config = runtimePiConfig(invocation.config);
    this.#observeEgress('provider');
    const extensions = await this.#extensions.resolve(config, signal);
    const identity = await this.#runtime.resolveIdentity(signal);
    const models = parsePiModels(
      await this.#runtime.runResolved(
        identity,
        ['--list-models', ...identity.safetyFlags, ...extensions.args],
        null,
        signal,
        DEFAULT_TIMEOUT_MS,
      ),
    );
    if (models.length > 0 && !models.some(({ id }) => id === config.modelId))
      throw new ProviderError('MODEL_NOT_FOUND');
    const output = await this.#runtime.runResolved(
      identity,
      [
        '-p',
        '--model',
        config.modelId,
        '--thinking',
        config.thinking,
        ...identity.safetyFlags,
        ...extensions.args,
      ],
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
    const config = basePiConfig(invocation.config);
    this.#observeEgress('provider');
    const extensions = await this.#extensions.resolve(config, signal);
    const identity = await this.#runtime.resolveIdentity(signal);
    const key = JSON.stringify([identityKey(identity), extensions.cacheKey]);
    if (
      invocation.refreshModels !== true &&
      this.#models?.key === key &&
      this.#models.expiresAt > this.#now()
    )
      return this.#models.models;
    const models = parsePiModels(
      await this.#runtime.runResolved(
        identity,
        ['--list-models', ...identity.safetyFlags, ...extensions.args],
        null,
        signal,
        DEFAULT_TIMEOUT_MS,
      ),
    );
    this.#models = { key, models, expiresAt: this.#now() + MODEL_CACHE_TTL_MS };
    return models;
  }

  capabilities(): VisionCapability {
    return 'unsupported';
  }

  async prepareCompletion(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<PreparedProviderCompletion | null> {
    const config = freezePiConfig(runtimePiConfig(invocation.config));
    // Egress observation must precede extension filesystem work and process work.
    this.#observeEgress('provider');
    return await this.#prepared.prepare(config, signal);
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    requestInput: ProviderCompletionRequest,
    signal: AbortSignal,
  ): Promise<string> {
    const config = runtimePiConfig(invocation.config);
    const request = ProviderCompletionRequestSchema.parse(requestInput);
    if (request.image !== undefined) throw new ProviderError('INVALID_CONFIG');
    const modelId = request.modelId ?? config.modelId;
    assertModelId(modelId);
    this.#observeEgress('provider');
    const extensions = await this.#extensions.resolve(config, signal);
    try {
      const identity = await this.#runtime.resolveIdentity(signal);
      const output = await this.#runtime.runResolved(
        identity,
        [
          '-p',
          '--model',
          modelId,
          '--thinking',
          config.thinking,
          ...identity.safetyFlags,
          ...extensions.args,
        ],
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
    basePiConfig(invocation.config);
    return signal.aborted
      ? Promise.reject(new ProviderError('CANCELLED'))
      : Promise.resolve('cloud');
  }
}
