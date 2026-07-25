import {
  MAX_PROVIDER_INPUT_UTF8_BYTES,
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
  ProviderConfigSchema,
  ProviderIdSchema,
  type Destination,
  type ModelInfo,
  type ProviderCatalogEntry,
  type ProviderCompletionRequest,
  type ProviderConfig,
  type ProviderId,
  type ProviderValidationResult,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type {
  CredentialResolver,
  ProviderCredentialPolicy,
  ProviderInvocationConfig,
} from './contracts';
import { parseAwsCredentials } from './aws-sigv4';
import { ProviderError, toProviderError } from './errors';
import type { ProviderRegistry } from './registry';

const PROVIDER_OPERATION_TIMEOUT_MS = 30_000;
/** One end-to-end budget for every Pi operation, including discovery, validation, execution, and cleanup. */
export const PI_PROVIDER_OPERATION_TIMEOUT_MS = 120_000;
export const PI_TERMINATION_RESERVE_MS = 5_000;
export const PI_MIN_OPERATION_TIMEOUT_MS = PI_TERMINATION_RESERVE_MS + 500;

export class ProviderService {
  readonly #registry: ProviderRegistry;
  readonly #credentials: CredentialResolver;
  readonly #operations = new Set<AbortController>();
  readonly #operationTimeoutMs: number;
  #disposed = false;

  constructor(
    registry: ProviderRegistry,
    credentials: CredentialResolver,
    options: { readonly operationTimeoutMs?: number } = {},
  ) {
    this.#registry = registry;
    this.#credentials = credentials;
    this.#operationTimeoutMs = options.operationTimeoutMs ?? PROVIDER_OPERATION_TIMEOUT_MS;
  }

  catalog(): readonly ProviderCatalogEntry[] {
    return this.#registry.catalog();
  }

  credentialPolicy(providerId: ProviderId): ProviderCredentialPolicy {
    try {
      return this.#registry.get(ProviderIdSchema.parse(providerId)).credentialPolicy;
    } catch (error: unknown) {
      throw toProviderError(error);
    }
  }

  credentialBinding(configInput: ProviderConfig): string {
    try {
      const config = ProviderConfigSchema.parse(configInput);
      return this.#registry.get(config.providerId).credentialBinding(config);
    } catch (error: unknown) {
      throw toProviderError(error);
    }
  }

  listModels(
    config: ProviderConfig,
    signal: AbortSignal,
    options: { readonly refresh?: boolean } = {},
  ): Promise<readonly ModelInfo[]> {
    return this.#run(config, signal, async (provider, invocation, operationSignal) =>
      removeCredentialEchoes(
        await provider.listModels(
          { ...invocation, ...(options.refresh === true ? { refreshModels: true } : {}) },
          operationSignal,
        ),
        sensitiveCredentialValues(provider.id, invocation.credential),
      ),
    );
  }

  testConnection(config: ProviderConfig, signal: AbortSignal): Promise<ProviderValidationResult> {
    return this.#run(config, signal, (provider, invocation, operationSignal) =>
      provider.validate(invocation, operationSignal),
    );
  }

  classifyDestination(config: ProviderConfig, signal: AbortSignal): Promise<Destination> {
    return this.#run(config, signal, (provider, invocation, operationSignal) =>
      provider.classifyDestination(invocation, operationSignal),
    );
  }

  cleanTranscript(
    config: ProviderConfig,
    request: ProviderCompletionRequest,
    signal: AbortSignal,
  ): Promise<string> {
    let parsedRequest: ReturnType<typeof ProviderCompletionRequestSchema.parse>;
    try {
      const untrustedInput: unknown = request.input;
      if (
        typeof untrustedInput === 'string' &&
        (untrustedInput.length > MAX_PROVIDER_INPUT_UTF8_BYTES ||
          new TextEncoder().encode(untrustedInput).byteLength > MAX_PROVIDER_INPUT_UTF8_BYTES)
      ) {
        throw new ProviderError('REQUEST_TOO_LARGE');
      }
      parsedRequest = ProviderCompletionRequestSchema.parse(request);
    } catch (error: unknown) {
      return Promise.reject(toProviderError(error));
    }
    return this.#run(config, signal, async (provider, invocation, operationSignal) => {
      const modelId = parsedRequest.modelId ?? config.modelId;
      if (
        parsedRequest.image !== undefined &&
        modelId !== undefined &&
        modelId !== null &&
        provider.capabilities(config, modelId) === 'unsupported'
      ) {
        throw new ProviderError('INVALID_CONFIG');
      }
      const output = await provider.cleanTranscript(invocation, parsedRequest, operationSignal);
      if (
        containsSensitiveValue(
          output,
          sensitiveCredentialValues(provider.id, invocation.credential),
        )
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return output;
    });
  }

  preflightCapability(
    config: ProviderConfig,
    modelId: string,
    signal: AbortSignal,
  ): Promise<VisionCapability> {
    return this.#run(config, signal, (provider, invocation, operationSignal) =>
      provider.capabilityPreflight === undefined
        ? Promise.resolve(provider.capabilities(config, modelId))
        : provider.capabilityPreflight(invocation, modelId, operationSignal),
    );
  }

  capabilities(config: ProviderConfig, modelId: string): VisionCapability {
    try {
      const parsed = ProviderConfigSchema.parse(config);
      const id = ProviderIdSchema.parse(parsed.providerId);
      if (modelId.trim().length === 0 || modelId.length > 512) {
        throw new ProviderError('INVALID_CONFIG');
      }
      return this.#registry.get(id).capabilities(parsed, modelId);
    } catch (error: unknown) {
      throw toProviderError(error);
    }
  }

  abortAll(): void {
    for (const controller of this.#operations) controller.abort();
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.abortAll();
  }

  async #run<Result>(
    configInput: ProviderConfig,
    callerSignal: AbortSignal,
    operation: (
      provider: ReturnType<ProviderRegistry['get']>,
      invocation: ProviderInvocationConfig,
      signal: AbortSignal,
    ) => Promise<Result>,
  ): Promise<Result> {
    if (this.#disposed) throw new ProviderError('UNAVAILABLE');
    if (callerSignal.aborted) throw new ProviderError('CANCELLED');
    let config: ProviderConfig;
    let id: ProviderId;
    try {
      config = ProviderConfigSchema.parse(configInput);
      id = ProviderIdSchema.parse(config.providerId);
    } catch (error: unknown) {
      throw toProviderError(error);
    }
    const controller = new AbortController();
    const timeoutState = { expired: false };
    const abort = (): void => controller.abort();
    const requestedTimeoutMs =
      config.timeoutMs ??
      (id === 'pi' && this.#operationTimeoutMs === PROVIDER_OPERATION_TIMEOUT_MS
        ? PI_PROVIDER_OPERATION_TIMEOUT_MS
        : this.#operationTimeoutMs);
    const operationTimeoutMs =
      id === 'pi' ? Math.max(requestedTimeoutMs, PI_MIN_OPERATION_TIMEOUT_MS) : requestedTimeoutMs;
    // Every Pi deadline reserves time for process-tree termination and exit confirmation. The
    // provider promise, rather than a racing hard timer, settles only after that cleanup finishes.
    const abortAfterMs =
      id === 'pi' ? operationTimeoutMs - PI_TERMINATION_RESERVE_MS : operationTimeoutMs;
    const earlyTimer = setTimeout(() => {
      timeoutState.expired = true;
      controller.abort();
    }, abortAfterMs);
    let hardTimer: ReturnType<typeof setTimeout> | undefined;
    callerSignal.addEventListener('abort', abort, { once: true });
    this.#operations.add(controller);
    const underlying = (async (): Promise<Result> => {
      try {
        const provider = this.#registry.get(id);
        const credential =
          provider.credentialPolicy === 'none'
            ? null
            : await waitForAbort(
                Promise.resolve(
                  this.#credentials.getCredential(id, provider.credentialBinding(config)),
                ),
                controller.signal,
              );
        if (controller.signal.aborted) throw new ProviderError('CANCELLED');
        return await operation(provider, { config, credential }, controller.signal);
      } catch (error: unknown) {
        if (timeoutState.expired && !isAborted(callerSignal)) throw new ProviderError('TIMEOUT');
        throw toProviderError(error);
      } finally {
        clearTimeout(earlyTimer);
        if (hardTimer !== undefined) clearTimeout(hardTimer);
        callerSignal.removeEventListener('abort', abort);
        this.#operations.delete(controller);
      }
    })();
    if (id === 'pi') return await underlying;
    const hardDeadline = new Promise<never>((_resolve, reject) => {
      hardTimer = setTimeout(() => {
        timeoutState.expired = true;
        controller.abort();
        reject(new ProviderError('TIMEOUT'));
      }, operationTimeoutMs);
    });
    return await Promise.race([underlying, hardDeadline]);
  }
}

function removeCredentialEchoes(
  models: readonly ModelInfo[],
  sensitiveValues: readonly string[],
): readonly ModelInfo[] {
  const safeModels = models.filter(
    (model) =>
      !containsSensitiveValue(model.id, sensitiveValues) &&
      !containsSensitiveValue(model.name, sensitiveValues),
  );
  if (models.length > 0 && safeModels.length === 0) throw new ProviderError('INVALID_RESPONSE');
  return Object.freeze(safeModels.map((model) => ModelInfoSchema.parse(model)));
}

function sensitiveCredentialValues(
  providerId: ProviderId,
  credential: string | null,
): readonly string[] {
  if (credential === null || credential.length < 8) return Object.freeze([]);
  if (providerId !== 'bedrock') return Object.freeze([credential]);
  try {
    const parsed = parseAwsCredentials(credential);
    return Object.freeze([
      parsed.accessKeyId,
      parsed.secretAccessKey,
      ...(parsed.sessionToken === undefined ? [] : [parsed.sessionToken]),
    ]);
  } catch {
    return Object.freeze([credential]);
  }
}

function containsSensitiveValue(value: string, sensitiveValues: readonly string[]): boolean {
  return sensitiveValues.some((sensitive) => sensitive.length >= 8 && value.includes(sensitive));
}

function isAborted(signal: AbortSignal): boolean {
  return signal.aborted;
}

function waitForAbort<Result>(operation: Promise<Result>, signal: AbortSignal): Promise<Result> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  return new Promise<Result>((resolve, reject) => {
    const abort = (): void => reject(new ProviderError('CANCELLED'));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (result) => {
        signal.removeEventListener('abort', abort);
        resolve(result);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
