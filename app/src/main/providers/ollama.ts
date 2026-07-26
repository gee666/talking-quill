import { createHash } from 'node:crypto';
import {
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
  ProviderConfigSchema,
  ProviderValidationResultSchema,
  type Destination,
  type ModelInfo,
  type ProviderConfig,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import { ProviderError } from './errors';
import type { JsonTransport } from './json-transport';

const FALLBACK_CONTEXT_WINDOW = 4_096;
const AUTOMATIC_CONTEXT_CAP = 16_384;
const MAX_MODELS = 64;
const MAX_MODEL_SUMMARIES = 1_000;
const MAX_CAPABILITIES = 64;
const MAX_MODEL_INFO_ENTRIES = 4_096;
const MAX_DETAILS_CACHE_ENTRIES = 128;
const DETAILS_CACHE_TTL_MS = 5 * 60_000;
const MODEL_DETAIL_CONCURRENCY = 4;

interface ModelDetails {
  readonly contextWindow: number | null;
  readonly vision: VisionCapability;
  readonly embedding: boolean;
}

interface CachedModelDetails {
  readonly details: ModelDetails;
  readonly expiresAt: number;
}

export class OllamaProvider implements SmartProvider {
  readonly id = 'ollama' as const;
  readonly credentialPolicy = 'optional' as const;
  readonly #transport: JsonTransport;
  readonly #endpointOverride: string | undefined;
  readonly #now: () => number;
  readonly #details = new Map<string, CachedModelDetails>();

  constructor(
    transport: JsonTransport,
    options: { readonly endpointOverride?: string; readonly now?: () => number } = {},
  ) {
    this.#transport = transport;
    this.#endpointOverride = options.endpointOverride;
    this.#now = options.now ?? Date.now;
  }

  credentialBinding(config: ProviderConfig): string {
    this.#validateConfig(config);
    return this.#baseUrl(config).href;
  }

  async validate(invocation: ProviderInvocationConfig, signal: AbortSignal) {
    this.#validateInvocation(invocation);
    const models = await this.listModels(invocation, signal);
    const selected = invocation.config.modelId;
    if (selected === undefined || selected === null) {
      throw new ProviderError('INVALID_CONFIG');
    }
    if (!models.some((model) => model.id === selected)) {
      throw new ProviderError('MODEL_NOT_FOUND');
    }
    // Validation must exercise the same selected model used by Task 9 transcript cleanup.
    await this.cleanTranscript(
      invocation,
      { input: 'Reply with OK.', modelId: selected, temperature: 0, maxOutputTokens: 8 },
      signal,
    );
    return ProviderValidationResultSchema.parse({
      ok: true,
      destination: await this.classifyDestination(invocation, signal),
      modelCount: models.length,
    });
  }

  async listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    this.#validateInvocation(invocation);
    const response = await this.#transport.request({
      url: new URL('/api/tags', this.#baseUrl(invocation.config)),
      method: 'GET',
      kind: 'model-list',
      headers: authHeaders(invocation.credential),
      credentialed: invocation.credential !== null,
      signal,
      maxResponseBytes: 4 * 1024 * 1024,
    });
    const record = asRecord(response.body);
    if (
      record === null ||
      !Array.isArray(record.models) ||
      record.models.length > MAX_MODEL_SUMMARIES
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (record.models.length === 0) throw new ProviderError('NO_MODELS');

    const models: ModelInfo[] = [];
    const seen = new Set<string>();
    const names: string[] = [];
    for (const item of record.models) {
      throwIfAborted(signal);
      const summary = asRecord(item);
      const name = readString(summary?.name ?? summary?.model);
      if (name === null || seen.has(name)) continue;
      seen.add(name);
      names.push(name);
    }
    const discoveryOrder = new Map(names.map((name, index) => [name, index] as const));
    const selectedModel = invocation.config.modelId;
    if (typeof selectedModel === 'string') {
      const selectedIndex = names.indexOf(selectedModel);
      if (selectedIndex > 0) {
        names.splice(selectedIndex, 1);
        names.unshift(selectedModel);
      }
    }
    const discovered = await mapWithConcurrency(
      names.slice(0, MAX_MODELS),
      MODEL_DETAIL_CONCURRENCY,
      signal,
      async (name, operationSignal) => {
        throwIfAborted(operationSignal);
        const details = await this.#showModel(
          invocation,
          name,
          operationSignal,
          invocation.refreshModels === true,
        );
        return details.embedding
          ? null
          : ModelInfoSchema.parse({
              id: name,
              name,
              contextWindow: details.contextWindow ?? FALLBACK_CONTEXT_WINDOW,
              vision: details.vision,
            });
      },
    );
    models.push(...discovered.filter((model): model is ModelInfo => model !== null));
    if (models.length === 0) throw new ProviderError('NO_MODELS');
    models.sort(
      (left, right) =>
        (discoveryOrder.get(left.id) ?? Number.MAX_SAFE_INTEGER) -
        (discoveryOrder.get(right.id) ?? Number.MAX_SAFE_INTEGER),
    );
    return Object.freeze(models);
  }

  capabilities(config: ProviderConfig, modelId: string): VisionCapability {
    const cached = this.#readCachedDetails(this.#capabilityKey(config, modelId, null));
    return cached?.vision ?? 'unknown';
  }

  async capabilityPreflight(
    invocation: ProviderInvocationConfig,
    modelId: string,
    signal: AbortSignal,
  ): Promise<VisionCapability> {
    this.#validateInvocation(invocation);
    const details = await this.#showModel(invocation, modelId, signal);
    return details.embedding ? 'unsupported' : details.vision;
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    requestInput: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
  ): Promise<string> {
    this.#validateInvocation(invocation);
    const request = parseCompletionRequest(requestInput);
    const model = request.modelId ?? invocation.config.modelId;
    if (model === undefined || model === null) throw new ProviderError('INVALID_CONFIG');
    // Image eligibility is authoritative only after a bounded /api/show probe. Names such as
    // "llava" never override explicit non-vision metadata, and an expired cache fails closed.
    const details = await this.#showModel(invocation, model, signal);
    if (details.embedding) throw new ProviderError('MODEL_NOT_FOUND');
    if (request.image !== undefined && details.vision !== 'supported') {
      throw new ProviderError('INVALID_CONFIG');
    }
    const detectedContext = details.contextWindow;
    const contextWindow =
      invocation.config.contextWindow === undefined
        ? detectedContext === null
          ? FALLBACK_CONTEXT_WINDOW
          : Math.min(detectedContext, AUTOMATIC_CONTEXT_CAP)
        : detectedContext === null
          ? invocation.config.contextWindow
          : Math.min(invocation.config.contextWindow, detectedContext);
    const maxOutputTokens = Math.min(
      request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 2_048,
      contextWindow,
    );
    const response = await this.#transport.request({
      url: new URL('/api/chat', this.#baseUrl(invocation.config)),
      method: 'POST',
      kind: 'completion',
      headers: authHeaders(invocation.credential),
      credentialed: invocation.credential !== null,
      body: Object.freeze({
        model,
        messages: Object.freeze([
          Object.freeze({
            role: 'user',
            content: request.input,
            ...(request.image === undefined
              ? {}
              : { images: Object.freeze([request.image.base64]) }),
          }),
        ]),
        stream: false,
        keep_alive: invocation.config.keepAlive ?? '5m',
        options: Object.freeze({
          num_ctx: contextWindow,
          temperature: request.temperature,
          num_predict: maxOutputTokens,
        }),
      }),
      signal,
      maxResponseBytes: 2 * 1024 * 1024,
    });
    return parseChatOutput(response.body);
  }

  classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination> {
    this.#validateInvocation(invocation);
    return this.#transport.classify(
      this.#baseUrl(invocation.config),
      { credentialed: invocation.credential !== null },
      signal,
    );
  }

  async #showModel(
    invocation: ProviderInvocationConfig,
    model: string,
    signal: AbortSignal,
    refresh = false,
  ): Promise<ModelDetails> {
    const cacheKey = this.#capabilityKey(invocation.config, model, invocation.credential);
    if (!refresh) {
      const cached = this.#readCachedDetails(cacheKey);
      if (cached !== null) return cached;
    }
    const response = await this.#transport.request({
      url: new URL('/api/show', this.#baseUrl(invocation.config)),
      method: 'POST',
      kind: 'model-detail',
      headers: authHeaders(invocation.credential),
      credentialed: invocation.credential !== null,
      body: Object.freeze({ model }),
      signal,
      maxResponseBytes: 4 * 1024 * 1024,
    });
    const record = asRecord(response.body);
    if (record === null) throw new ProviderError('INVALID_RESPONSE');
    if (
      record.capabilities !== undefined &&
      (!Array.isArray(record.capabilities) || record.capabilities.length > MAX_CAPABILITIES)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const capabilities = Array.isArray(record.capabilities)
      ? record.capabilities.map(readCapability)
      : [];
    const normalizedCapabilities = capabilities.map((value) => value.toLowerCase());
    const vision: VisionCapability = normalizedCapabilities.includes('vision')
      ? 'supported'
      : capabilities.length > 0
        ? 'unsupported'
        : 'unknown';
    const modelInfo = asRecord(record.model_info);
    if (modelInfo !== null && Object.keys(modelInfo).length > MAX_MODEL_INFO_ENTRIES) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    let contextWindow: number | null = null;
    if (modelInfo !== null) {
      for (const [key, value] of Object.entries(modelInfo)) {
        if (!key.endsWith('.context_length')) continue;
        const parsed = readPositiveInteger(value);
        if (parsed !== null) {
          contextWindow = parsed;
          break;
        }
      }
    }
    const details = Object.freeze({
      contextWindow,
      vision,
      embedding: normalizedCapabilities.includes('embedding'),
    });
    this.#cacheDetails(cacheKey, details);
    return details;
  }

  #cacheDetails(key: string, details: ModelDetails): void {
    this.#details.delete(key);
    this.#details.set(key, { details, expiresAt: this.#now() + DETAILS_CACHE_TTL_MS });
    while (this.#details.size > MAX_DETAILS_CACHE_ENTRIES) {
      const oldest = this.#details.keys().next().value;
      if (oldest === undefined) break;
      this.#details.delete(oldest);
    }
  }

  #readCachedDetails(key: string): ModelDetails | null {
    const cached = this.#details.get(key);
    if (cached === undefined) return null;
    if (cached.expiresAt <= this.#now()) {
      this.#details.delete(key);
      return null;
    }
    this.#details.delete(key);
    this.#details.set(key, cached);
    return cached.details;
  }

  #capabilityKey(config: ProviderConfig, model: string, credential: string | null): string {
    const credentialBinding =
      credential === null
        ? 'none'
        : createHash('sha256').update(credential, 'utf8').digest('base64url');
    return `${this.#baseUrl(config).href}\n${credentialBinding}\n${model}`;
  }

  #validateConfig(configInput: ProviderConfig): void {
    const config = parseProviderConfig(configInput);
    if (config.providerId !== this.id) throw new ProviderError('INVALID_CONFIG');
  }

  #validateInvocation(invocation: ProviderInvocationConfig): void {
    this.#validateConfig(invocation.config);
    if (
      invocation.credential !== null &&
      (invocation.credential.length === 0 ||
        invocation.credential.length > 16_384 ||
        invocation.credential.trim() !== invocation.credential ||
        hasControlCharacters(invocation.credential))
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
  }

  #baseUrl(config: ProviderConfig): URL {
    const source = this.#endpointOverride ?? config.baseUrl;
    if (source === undefined) throw new ProviderError('INVALID_CONFIG');
    let url: URL;
    try {
      url = new URL(source);
    } catch {
      throw new ProviderError('INVALID_CONFIG');
    }
    if (
      url.username.length > 0 ||
      url.password.length > 0 ||
      url.search.length > 0 ||
      url.hash.length > 0
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    url.pathname = '/';
    return url;
  }
}

function parseProviderConfig(input: unknown): ProviderConfig {
  try {
    return ProviderConfigSchema.parse(input);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

function parseCompletionRequest(
  input: unknown,
): ReturnType<typeof ProviderCompletionRequestSchema.parse> {
  try {
    return ProviderCompletionRequestSchema.parse(input);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

function authHeaders(credential: string | null): Readonly<Record<string, string>> {
  return credential === null
    ? Object.freeze({})
    : Object.freeze({ authorization: `Bearer ${credential}` });
}

function asRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}

function readString(input: unknown): string | null {
  return typeof input === 'string' && input.trim().length > 0 && input.length <= 512
    ? input.trim()
    : null;
}

function readCapability(input: unknown): string {
  if (
    typeof input !== 'string' ||
    input.length === 0 ||
    input.length > 128 ||
    hasControlCharacters(input)
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return input;
}

function parseChatOutput(input: unknown): string {
  const envelope = asRecord(input);
  const message = asRecord(envelope?.message);
  const content = message?.content;
  if (
    envelope?.done !== true ||
    (envelope.done_reason !== undefined &&
      envelope.done_reason !== null &&
      envelope.done_reason !== 'stop') ||
    message?.role !== 'assistant' ||
    typeof content !== 'string' ||
    content.trim().length === 0 ||
    content.length > 200_000
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return content;
}

function readPositiveInteger(input: unknown): number | null {
  return typeof input === 'number' && Number.isInteger(input) && input > 0 && input <= 2_000_000
    ? input
    : null;
}

function hasControlCharacters(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

async function mapWithConcurrency<Input, Output>(
  values: readonly Input[],
  concurrency: number,
  signal: AbortSignal,
  operation: (value: Input, signal: AbortSignal) => Promise<Output>,
): Promise<Output[]> {
  const results = new Array<Output>(values.length);
  const controller = new AbortController();
  const operationSignal = AbortSignal.any([signal, controller.signal]);
  let nextIndex = 0;
  const failure: { occurred: boolean; error: unknown } = { occurred: false, error: null };
  const worker = async (): Promise<void> => {
    for (;;) {
      if (failure.occurred || operationSignal.aborted) return;
      const index = nextIndex;
      nextIndex += 1;
      const value = values[index];
      if (value === undefined) return;
      try {
        results[index] = await operation(value, operationSignal);
      } catch (error: unknown) {
        if (controller.signal.aborted) return;
        failure.occurred = true;
        failure.error = error;
        controller.abort(error);
        return;
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, values.length) }, () => worker()));
  if (failure.occurred) {
    if (failure.error instanceof Error) throw failure.error;
    throw new ProviderError('UNAVAILABLE');
  }
  throwIfAborted(signal);
  return results;
}

function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}
