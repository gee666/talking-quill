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
  PreparedCompletionCloseReason,
  PreparedCompletionLease,
  PreparedProviderCompletion,
  ProviderCredentialPolicy,
  ProviderInvocationConfig,
} from './contracts';
import { parseAwsCredentials } from './aws-sigv4';
import { ProviderError, toProviderError } from './errors';
import type { ProviderRegistry } from './registry';

const PROVIDER_OPERATION_TIMEOUT_MS = 30_000;
/** One end-to-end budget for every Pi operation, including discovery, validation, execution, and cleanup. */
export const PI_PROVIDER_OPERATION_TIMEOUT_MS = 120_000;
export const PI_TERMINATION_RESERVE_MS = 6_500;
export const PI_MIN_OPERATION_TIMEOUT_MS = PI_TERMINATION_RESERVE_MS + 500;

type RegistryProvider = ReturnType<ProviderRegistry['get']>;
type ParsedCompletionRequest = ReturnType<typeof ProviderCompletionRequestSchema.parse>;

interface ProviderOperationContext {
  readonly config: ProviderConfig;
  readonly id: ProviderId;
  readonly operation: TrackedProviderOperation;
  readonly timeoutState: { expired: boolean };
  readonly operationTimeoutMs: number;
  finish(): void;
}

interface AdoptedPreparedCompletion {
  readonly closed: Promise<void>;
  isClosed(): boolean;
}

class TrackedProviderOperation {
  readonly controller = new AbortController();
  readonly settled: Promise<void>;
  readonly #resolveSettled: () => void;
  readonly #onFinish: () => void;
  #close: ((reason: PreparedCompletionCloseReason) => void) | null = null;
  #closeReason: PreparedCompletionCloseReason | null = null;
  #finished = false;

  constructor(onFinish: () => void) {
    this.#onFinish = onFinish;
    let resolveSettled!: () => void;
    this.settled = new Promise<void>((resolve) => {
      resolveSettled = resolve;
    });
    this.#resolveSettled = resolveSettled;
  }

  requestClose(reason: PreparedCompletionCloseReason): void {
    this.#closeReason ??= reason;
    if (!this.controller.signal.aborted) this.controller.abort();
    this.#dispatchClose();
  }

  bindClose(close: (reason: PreparedCompletionCloseReason) => void): void {
    this.#close = close;
    this.#dispatchClose();
  }

  finish(): void {
    if (this.#finished) return;
    this.#finished = true;
    this.#onFinish();
    this.#resolveSettled();
  }

  #dispatchClose(): void {
    if (this.#close === null || this.#closeReason === null) return;
    const close = this.#close;
    const reason = this.#closeReason;
    this.#close = null;
    close(reason);
  }
}

export class ProviderService {
  readonly #registry: ProviderRegistry;
  readonly #credentials: CredentialResolver;
  readonly #operations = new Set<TrackedProviderOperation>();
  readonly #operationTimeoutMs: number;
  #accepting = true;
  #disposed = false;
  #drainError: ProviderError | null = null;

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
    let parsedRequest: ParsedCompletionRequest;
    try {
      parsedRequest = parseCompletionRequest(request);
    } catch (error: unknown) {
      return Promise.reject(toProviderError(error));
    }
    return this.#run(config, signal, async (provider, invocation, operationSignal) => {
      assertCompletionCapability(provider, invocation.config, parsedRequest);
      const output = await provider.cleanTranscript(invocation, parsedRequest, operationSignal);
      return validateCompletionOutput(provider.id, invocation.credential, output);
    });
  }

  /**
   * Acquires an optional provider-owned completion lease. Unsupported providers return null and
   * keep the existing cleanTranscript path unchanged.
   */
  async prepareCompletion(
    configInput: ProviderConfig,
    callerSignal: AbortSignal,
  ): Promise<PreparedCompletionLease | null> {
    const context = this.#openOperation(configInput, callerSignal);
    let provider: RegistryProvider;
    let invocation: ProviderInvocationConfig;
    try {
      provider = this.#registry.get(context.id);
      if (provider.prepareCompletion === undefined) {
        context.finish();
        return null;
      }
      const credential =
        provider.credentialPolicy === 'none'
          ? null
          : await waitForAbort(
              Promise.resolve(
                this.#credentials.getCredential(
                  context.id,
                  provider.credentialBinding(context.config),
                ),
              ),
              context.operation.controller.signal,
            );
      if (context.operation.controller.signal.aborted) throw new ProviderError('CANCELLED');
      invocation = { config: context.config, credential };
    } catch (error: unknown) {
      context.finish();
      throw preparedOperationError(error, context, callerSignal, true);
    }

    const preparation = Promise.resolve().then(
      () => provider.prepareCompletion?.(invocation, context.operation.controller.signal) ?? null,
    );
    let prepared: PreparedProviderCompletion | null;
    try {
      prepared = await waitForAbort(preparation, context.operation.controller.signal);
    } catch (error: unknown) {
      if (isAborted(context.operation.controller.signal)) {
        this.#retireLatePreparation(context, preparation);
      } else {
        context.finish();
      }
      throw preparedOperationError(error, context, callerSignal, true);
    }
    if (prepared === null) {
      context.finish();
      return null;
    }

    const adopted = this.#adoptPreparedCompletion(context, prepared);
    // Observe a capability that was already closed before exposing it to a caller.
    await Promise.resolve();
    if (adopted.isClosed()) {
      try {
        await adopted.closed;
      } catch (error: unknown) {
        throw preparedOperationError(error, context, callerSignal, true);
      }
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
    if (isAborted(context.operation.controller.signal)) {
      // Service tracking retains ownership of retirement after the caller is released.
      throw preparedOperationError(new ProviderError('CANCELLED'), context, callerSignal, true);
    }
    return this.#createPreparedLease(context, provider, invocation, prepared, adopted);
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

  stopAccepting(): void {
    this.#accepting = false;
  }

  abortAll(reason: PreparedCompletionCloseReason = 'cancelled'): void {
    for (const operation of this.#operations) operation.requestClose(reason);
  }

  async drain(): Promise<void> {
    while (this.#operations.size > 0) {
      await Promise.all([...this.#operations].map(({ settled }) => settled));
    }
    if (this.#drainError !== null) throw this.#drainError;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.stopAccepting();
    this.abortAll('shutdown');
  }

  async #run<Result>(
    configInput: ProviderConfig,
    callerSignal: AbortSignal,
    operation: (
      provider: RegistryProvider,
      invocation: ProviderInvocationConfig,
      signal: AbortSignal,
    ) => Promise<Result>,
  ): Promise<Result> {
    const context = this.#openOperation(configInput, callerSignal);
    let hardTimer: ReturnType<typeof setTimeout> | undefined;
    const underlying = (async (): Promise<Result> => {
      const provider = this.#registry.get(context.id);
      const credential =
        provider.credentialPolicy === 'none'
          ? null
          : await waitForAbort(
              Promise.resolve(
                this.#credentials.getCredential(
                  context.id,
                  provider.credentialBinding(context.config),
                ),
              ),
              context.operation.controller.signal,
            );
      if (context.operation.controller.signal.aborted) throw new ProviderError('CANCELLED');
      return await operation(
        provider,
        { config: context.config, credential },
        context.operation.controller.signal,
      );
    })();
    const normalized = underlying.catch((error: unknown) => {
      if (context.timeoutState.expired && !callerSignal.aborted) {
        throw new ProviderError('TIMEOUT');
      }
      throw toProviderError(error);
    });
    void underlying.then(
      () => {
        if (hardTimer !== undefined) clearTimeout(hardTimer);
        context.finish();
      },
      () => {
        if (hardTimer !== undefined) clearTimeout(hardTimer);
        context.finish();
      },
    );
    if (context.id === 'pi') return await normalized;
    const cancellable = waitForAbort(normalized, context.operation.controller.signal).catch(
      (error: unknown) => {
        if (context.timeoutState.expired && !callerSignal.aborted) {
          throw new ProviderError('TIMEOUT');
        }
        throw toProviderError(error);
      },
    );
    const hardDeadline = new Promise<never>((_resolve, reject) => {
      hardTimer = setTimeout(() => {
        context.timeoutState.expired = true;
        context.operation.requestClose('timeout');
        reject(new ProviderError('TIMEOUT'));
      }, context.operationTimeoutMs);
    });
    return await Promise.race([cancellable, hardDeadline]);
  }

  #openOperation(configInput: ProviderConfig, callerSignal: AbortSignal): ProviderOperationContext {
    if (!this.#accepting) throw new ProviderError('UNAVAILABLE');
    if (callerSignal.aborted) throw new ProviderError('CANCELLED');
    let config: ProviderConfig;
    let id: ProviderId;
    try {
      config = ProviderConfigSchema.parse(configInput);
      id = ProviderIdSchema.parse(config.providerId);
    } catch (error: unknown) {
      throw toProviderError(error);
    }
    const operation = new TrackedProviderOperation(() => this.#operations.delete(operation));
    this.#operations.add(operation);
    const timeoutState = { expired: false };
    const requestedTimeoutMs =
      config.timeoutMs ??
      (id === 'pi' && this.#operationTimeoutMs === PROVIDER_OPERATION_TIMEOUT_MS
        ? PI_PROVIDER_OPERATION_TIMEOUT_MS
        : this.#operationTimeoutMs);
    const operationTimeoutMs =
      id === 'pi' ? Math.max(requestedTimeoutMs, PI_MIN_OPERATION_TIMEOUT_MS) : requestedTimeoutMs;
    // Every Pi deadline reserves time for process-tree termination and exit confirmation.
    const abortAfterMs =
      id === 'pi' ? operationTimeoutMs - PI_TERMINATION_RESERVE_MS : operationTimeoutMs;
    const earlyTimer = setTimeout(() => {
      timeoutState.expired = true;
      operation.requestClose('timeout');
    }, abortAfterMs);
    const abort = (): void => operation.requestClose('cancelled');
    callerSignal.addEventListener('abort', abort, { once: true });
    let finished = false;
    return {
      config,
      id,
      operation,
      timeoutState,
      operationTimeoutMs,
      finish: () => {
        if (finished) return;
        finished = true;
        clearTimeout(earlyTimer);
        callerSignal.removeEventListener('abort', abort);
        operation.finish();
      },
    };
  }

  #createPreparedLease(
    context: ProviderOperationContext,
    provider: RegistryProvider,
    invocation: ProviderInvocationConfig,
    prepared: PreparedProviderCompletion,
    adopted: AdoptedPreparedCompletion,
  ): PreparedCompletionLease {
    let consumed = false;
    const requestClose = (reason: PreparedCompletionCloseReason): void => {
      context.operation.requestClose(reason);
    };
    const complete = async (
      requestInput: ProviderCompletionRequest,
      callerSignal: AbortSignal,
    ): Promise<string> => {
      if (consumed) throw new ProviderError('INVALID_CONFIG');
      consumed = true;
      let providerInvoked = false;
      let completionController: AbortController | null = null;
      let abortCompletion: (() => void) | null = null;
      try {
        const request = parseCompletionRequest(requestInput);
        assertCompletionCapability(provider, invocation.config, request);
        // Promise settlement callbacks run before this continuation, closing the resolve-and-use gap.
        await Promise.resolve();
        if (adopted.isClosed()) {
          throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
        }
        if (callerSignal.aborted || context.operation.controller.signal.aborted) {
          throw new ProviderError('CANCELLED', { fallbackEligible: true });
        }
        completionController = new AbortController();
        abortCompletion = () => completionController?.abort();
        callerSignal.addEventListener('abort', abortCompletion, { once: true });
        context.operation.controller.signal.addEventListener('abort', abortCompletion, {
          once: true,
        });
        providerInvoked = true;
        const output = await waitForAbort(
          Promise.resolve(prepared.complete(request, completionController.signal)),
          completionController.signal,
        );
        const validated = validateCompletionOutput(provider.id, invocation.credential, output);
        requestClose('completed');
        return validated;
      } catch (error: unknown) {
        const mapped = preparedOperationError(error, context, callerSignal, !providerInvoked);
        requestClose(
          mapped.code === 'TIMEOUT'
            ? 'timeout'
            : mapped.code === 'CANCELLED'
              ? 'cancelled'
              : 'failed',
        );
        throw mapped;
      } finally {
        if (abortCompletion !== null) {
          callerSignal.removeEventListener('abort', abortCompletion);
          context.operation.controller.signal.removeEventListener('abort', abortCompletion);
        }
      }
    };
    return Object.freeze({ complete, requestClose, closed: adopted.closed });
  }

  #adoptPreparedCompletion(
    context: ProviderOperationContext,
    prepared: PreparedProviderCompletion,
  ): AdoptedPreparedCompletion {
    context.operation.bindClose((reason) => {
      try {
        prepared.requestClose(reason);
      } catch (error: unknown) {
        this.#drainError ??= toProviderError(error);
      }
    });
    let providerClosed: Promise<void>;
    try {
      providerClosed = Promise.resolve(prepared.closed);
    } catch (error: unknown) {
      providerClosed = Promise.reject(toProviderError(error));
    }
    let closedSettled = false;
    const closed = providerClosed
      .then(
        () => {
          closedSettled = true;
        },
        (error: unknown) => {
          closedSettled = true;
          const normalized = toProviderError(error);
          this.#drainError ??= normalized;
          throw normalized;
        },
      )
      .finally(() => context.finish());
    // Service ownership may outlive both the preparer and the completion caller.
    void closed.catch(() => undefined);
    return Object.freeze({ closed, isClosed: () => closedSettled });
  }

  #retireLatePreparation(
    context: ProviderOperationContext,
    preparation: Promise<PreparedProviderCompletion | null>,
  ): void {
    void preparation.then(
      (prepared) => {
        if (prepared === null) {
          context.finish();
          return;
        }
        void this.#adoptPreparedCompletion(context, prepared);
      },
      () => context.finish(),
    );
  }
}

function parseCompletionRequest(request: ProviderCompletionRequest): ParsedCompletionRequest {
  const untrustedRequest: unknown = request;
  const untrustedInput: unknown =
    typeof untrustedRequest === 'object' && untrustedRequest !== null && 'input' in untrustedRequest
      ? untrustedRequest.input
      : undefined;
  if (
    typeof untrustedInput === 'string' &&
    (untrustedInput.length > MAX_PROVIDER_INPUT_UTF8_BYTES ||
      new TextEncoder().encode(untrustedInput).byteLength > MAX_PROVIDER_INPUT_UTF8_BYTES)
  ) {
    throw new ProviderError('REQUEST_TOO_LARGE');
  }
  return ProviderCompletionRequestSchema.parse(request);
}

function assertCompletionCapability(
  provider: RegistryProvider,
  config: ProviderConfig,
  request: ParsedCompletionRequest,
): void {
  const modelId = request.modelId ?? config.modelId;
  if (
    request.image !== undefined &&
    modelId !== undefined &&
    modelId !== null &&
    provider.capabilities(config, modelId) === 'unsupported'
  ) {
    throw new ProviderError('INVALID_CONFIG');
  }
}

function validateCompletionOutput(
  providerId: ProviderId,
  credential: string | null,
  output: unknown,
): string {
  if (typeof output !== 'string') throw new ProviderError('INVALID_RESPONSE');
  if (containsSensitiveValue(output, sensitiveCredentialValues(providerId, credential))) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return output;
}

function preparedOperationError(
  error: unknown,
  context: Pick<ProviderOperationContext, 'operation' | 'timeoutState'>,
  callerSignal: AbortSignal,
  fallbackEligible: boolean,
): ProviderError {
  const original = toProviderError(error);
  const code =
    context.timeoutState.expired && !callerSignal.aborted
      ? 'TIMEOUT'
      : callerSignal.aborted || context.operation.controller.signal.aborted
        ? 'CANCELLED'
        : original.code;
  return new ProviderError(code, {
    fallbackEligible: fallbackEligible || original.fallbackEligible,
  });
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
