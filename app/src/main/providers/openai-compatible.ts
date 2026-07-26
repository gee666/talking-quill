import { providerModelSelectionPolicy } from '../../shared/provider-model-selection';
import {
  ModelInfoSchema,
  ProviderConfigSchema,
  ProviderValidationResultSchema,
  type Destination,
  type ModelInfo,
  type ProviderConfig,
  type ProviderId,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import { ProviderError } from './errors';
import type { JsonTransport } from './json-transport';
import {
  createCompletionBody,
  parseCompletionOutput,
  parseCompletionRequest,
} from './openai-compatible-codecs';
import { prepareLemonadeCompletion } from './openai-compatible-lemonade';
import { normalizeModels } from './openai-compatible-models';
import type { OpenAICompatiblePreset } from './presets';
import { staticVisionCapability } from './vision-capabilities';

export interface OpenAICompatibleProviderOptions {
  /** Test-only endpoint substitution. Production registries leave this undefined. */
  readonly endpointOverride?: string;
}

export function createOpenAICompatibleProvider(
  preset: OpenAICompatiblePreset,
  transport: JsonTransport,
  options: OpenAICompatibleProviderOptions = {},
): SmartProvider {
  return new OpenAICompatibleProvider(preset, transport, options.endpointOverride);
}

class OpenAICompatibleProvider implements SmartProvider {
  readonly id;
  readonly credentialPolicy;
  readonly #preset: OpenAICompatiblePreset;
  readonly #transport: JsonTransport;
  readonly #endpointOverride: string | undefined;
  readonly #vision = new Map<string, VisionCapability>();

  constructor(preset: OpenAICompatiblePreset, transport: JsonTransport, endpointOverride?: string) {
    this.id = preset.id;
    this.credentialPolicy =
      preset.auth === 'none'
        ? ('none' as const)
        : preset.auth === 'required-bearer'
          ? ('required' as const)
          : ('optional' as const);
    this.#preset = preset;
    this.#transport = transport;
    this.#endpointOverride = endpointOverride;
  }

  credentialBinding(config: ProviderConfig): string {
    this.#validateConfig(config);
    return this.#baseUrl(config).href;
  }

  async validate(invocation: ProviderInvocationConfig, signal: AbortSignal) {
    this.#validateConfig(invocation.config);
    this.#validateCredential(invocation.credential, true);
    const models = await this.listModels(invocation, signal);
    const selectedModel = invocation.config.modelId;
    if (
      (selectedModel === undefined || selectedModel === null) &&
      providerModelSelectionPolicy(this.id) === 'required'
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    if (
      selectedModel !== undefined &&
      selectedModel !== null &&
      models.length > 0 &&
      !models.some((model) => model.id === selectedModel)
    ) {
      throw new ProviderError('MODEL_NOT_FOUND');
    }
    const probeModel = selectedModel ?? this.#preset.defaultModel;
    if (probeModel === null && providerModelSelectionPolicy(this.id) === 'required') {
      throw new ProviderError('INVALID_CONFIG');
    }
    const probe = await this.#complete(
      invocation,
      {
        input: 'Reply with OK.',
        ...(probeModel === null ? {} : { modelId: probeModel }),
        temperature: 0,
        maxOutputTokens: 8,
      },
      signal,
      { allowTokenExhaustedResponse: this.#preset.protocol === 'responses' },
    );
    const destination = probe.destination;
    return ProviderValidationResultSchema.parse({
      ok: true,
      destination,
      modelCount: models.length,
    });
  }

  async listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    this.#validateConfig(invocation.config);
    const source = this.#preset.modelList;
    this.#validateCredential(invocation.credential, source.kind === 'http' && !source.public);
    if (source.kind === 'none') return Object.freeze([]);
    if (source.kind === 'static') {
      const models = source.models.map((model) =>
        ModelInfoSchema.parse({
          id: model.id,
          name: model.id,
          contextWindow: model.contextWindow,
          vision: resolvedStaticVision(this.id, model.id, this.#preset.vision),
        }),
      );
      this.#rememberVision(invocation.config, models);
      return Object.freeze(models);
    }
    const baseUrl = this.#baseUrl(invocation.config);
    const response = await this.#transport.request({
      url: joinEndpoint(baseUrl, source.path),
      method: 'GET',
      kind: 'model-list',
      headers: source.public ? {} : this.#authHeaders(invocation.credential),
      credentialed: !source.public && invocation.credential !== null,
      fixedCloud: this.#isFixedCloud(),
      signal,
      ...(invocation.config.timeoutMs === undefined
        ? {}
        : { timeoutMs: invocation.config.timeoutMs }),
      maxResponseBytes: 4 * 1024 * 1024,
    });
    let models = normalizeModels(
      response.body,
      source.format,
      source.filter,
      invocation.config.contextWindow ?? this.#preset.defaultContextWindow,
      this.#preset.modelContextWindows ?? {},
      this.#preset.vision,
    );
    if (models.length === 0) throw new ProviderError('NO_MODELS');
    models = Object.freeze(
      models.map((model) =>
        model.vision === 'unknown'
          ? Object.freeze({
              ...model,
              vision: resolvedStaticVision(this.id, model.id, this.#preset.vision),
            })
          : model,
      ),
    );
    if (source.contextPath !== undefined) {
      try {
        const contextResponse = await this.#transport.request({
          url: joinEndpoint(baseUrl, source.contextPath),
          method: 'GET',
          kind: 'model-list',
          headers: source.public ? {} : this.#authHeaders(invocation.credential),
          credentialed: !source.public && invocation.credential !== null,
          fixedCloud: this.#isFixedCloud(),
          signal,
          ...(invocation.config.timeoutMs === undefined
            ? {}
            : { timeoutMs: invocation.config.timeoutMs }),
          maxResponseBytes: 4 * 1024 * 1024,
        });
        const contexts = normalizeModels(
          contextResponse.body,
          source.format,
          source.filter,
          invocation.config.contextWindow ?? this.#preset.defaultContextWindow,
          this.#preset.modelContextWindows ?? {},
          this.#preset.vision,
        );
        const contextsById = new Map(contexts.map((model) => [model.id, model.contextWindow]));
        models = Object.freeze(
          models.map((model) =>
            Object.freeze({
              ...model,
              contextWindow: contextsById.get(model.id) ?? model.contextWindow,
            }),
          ),
        );
      } catch (error: unknown) {
        if (!(error instanceof ProviderError)) throw error;
        if (error.code !== 'REMOTE_FAILURE' && error.code !== 'INVALID_RESPONSE') throw error;
        // LM Studio's richer context endpoint is optional and unavailable on older servers.
      }
    }
    this.#rememberVision(invocation.config, models);
    return models;
  }

  capabilities(config: ProviderConfig, modelId: string): VisionCapability {
    return (
      this.#vision.get(this.#visionKey(config, modelId)) ??
      resolvedStaticVision(this.id, modelId, this.#preset.vision)
    );
  }

  #rememberVision(config: ProviderConfig, models: readonly ModelInfo[]): void {
    for (const model of models) this.#vision.set(this.#visionKey(config, model.id), model.vision);
    while (this.#vision.size > 10_000) {
      const oldest = this.#vision.keys().next().value;
      if (oldest === undefined) break;
      this.#vision.delete(oldest);
    }
  }

  #visionKey(config: ProviderConfig, modelId: string): string {
    return `${this.#baseUrl(config).href}\n${modelId}`;
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    requestInput: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
  ): Promise<string> {
    return (await this.#complete(invocation, requestInput, signal)).text;
  }

  async #complete(
    invocation: ProviderInvocationConfig,
    requestInput: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
    options: { readonly allowTokenExhaustedResponse?: boolean } = {},
  ): Promise<{ readonly text: string; readonly destination: Destination }> {
    this.#validateConfig(invocation.config);
    this.#validateCredential(invocation.credential, true);
    const request = parseCompletionRequest(requestInput);
    const selectedModel = request.modelId ?? invocation.config.modelId;
    if (
      (selectedModel === undefined || selectedModel === null) &&
      providerModelSelectionPolicy(this.id) === 'required'
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    const model = selectedModel ?? this.#preset.defaultModel;
    const maxOutputTokens =
      request.maxOutputTokens ??
      invocation.config.maxOutputTokens ??
      this.#preset.defaultMaxOutputTokens;
    await this.#prepareCompletion(invocation, model, signal);
    const body = createCompletionBody(this.#preset, model, request, maxOutputTokens);
    const response = await this.#transport.request({
      url: joinEndpoint(this.#baseUrl(invocation.config), this.#preset.completionPath),
      method: 'POST',
      kind: 'completion',
      headers: this.#authHeaders(invocation.credential),
      body,
      credentialed: invocation.credential !== null,
      fixedCloud: this.#isFixedCloud(),
      signal,
      ...(invocation.config.timeoutMs === undefined
        ? {}
        : { timeoutMs: invocation.config.timeoutMs }),
      maxResponseBytes: 2 * 1024 * 1024,
    });
    const text = parseCompletionOutput(
      this.#preset.protocol,
      response.body,
      maxOutputTokens,
      options.allowTokenExhaustedResponse === true,
    );
    return Object.freeze({ text, destination: response.destination });
  }

  async #prepareCompletion(
    invocation: ProviderInvocationConfig,
    model: string | null | undefined,
    signal: AbortSignal,
  ): Promise<void> {
    if (this.#preset.preparation !== 'lemonade-load') return;
    let baseUrl: URL | undefined;
    await prepareLemonadeCompletion({
      model,
      contextWindow: invocation.config.contextWindow ?? this.#preset.defaultContextWindow,
      listInstalledModels: () => this.listModels(invocation, signal),
      endpoint: (path) => {
        baseUrl ??= this.#baseUrl(invocation.config);
        return joinEndpoint(baseUrl, path);
      },
      transport: this.#transport,
      headers: this.#authHeaders(invocation.credential),
      credentialed: invocation.credential !== null,
      fixedCloud: this.#isFixedCloud(),
      signal,
      ...(invocation.config.timeoutMs === undefined
        ? {}
        : { timeoutMs: invocation.config.timeoutMs }),
    });
  }

  async classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination> {
    this.#validateConfig(invocation.config);
    this.#validateCredential(invocation.credential, false);
    return this.#transport.classify(
      this.#baseUrl(invocation.config),
      { credentialed: invocation.credential !== null, fixedCloud: this.#isFixedCloud() },
      signal,
    );
  }

  #validateConfig(configInput: ProviderConfig): void {
    const config = parseProviderConfig(configInput);
    if (config.providerId !== this.id) throw new ProviderError('INVALID_CONFIG');
  }

  #validateCredential(credential: string | null, requiredForOperation: boolean): void {
    if (this.#preset.auth === 'none') return;
    if (credential !== null && !hasCredential(credential)) {
      throw new ProviderError('INVALID_CONFIG');
    }
    if (
      requiredForOperation &&
      this.#preset.auth === 'required-bearer' &&
      !hasCredential(credential)
    ) {
      throw new ProviderError('MISSING_CREDENTIAL');
    }
  }

  #baseUrl(config: ProviderConfig): URL {
    const raw =
      this.#endpointOverride ??
      (this.#preset.endpoint.kind === 'fixed' ? this.#preset.endpoint.value : config.baseUrl);
    if (raw === undefined) throw new ProviderError('INVALID_CONFIG');
    return normalizeBaseUrl(raw, this.#preset.endpoint.normalization);
  }

  #authHeaders(credential: string | null): Readonly<Record<string, string>> {
    if (this.#preset.auth === 'none' || credential === null) return Object.freeze({});
    return Object.freeze({ authorization: `Bearer ${credential}` });
  }

  #isFixedCloud(): boolean {
    return this.#endpointOverride === undefined && this.#preset.endpoint.kind === 'fixed';
  }
}

function resolvedStaticVision(
  providerId: ProviderId,
  modelId: string,
  preset: VisionCapability,
): VisionCapability {
  const known = staticVisionCapability(providerId, modelId);
  return known === 'unknown' ? preset : known;
}

function parseProviderConfig(input: unknown): ProviderConfig {
  try {
    return ProviderConfigSchema.parse(input);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

function hasCredential(value: string | null): value is string {
  return (
    value !== null &&
    value.trim().length > 0 &&
    value.trim() === value &&
    value.length <= 16_384 &&
    !/[\r\n]/.test(value)
  );
}

export function normalizeBaseUrl(
  raw: string,
  normalization: OpenAICompatiblePreset['endpoint']['normalization'],
): URL {
  let input: URL;
  try {
    input = new URL(raw);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
  if (
    input.username.length > 0 ||
    input.password.length > 0 ||
    input.search.length > 0 ||
    input.hash.length > 0
  ) {
    throw new ProviderError('INVALID_CONFIG');
  }
  if (normalization === 'origin-v1') return new URL('/v1/', input.origin);
  if (normalization === 'origin-api-v1') return new URL('/api/v1/', input.origin);
  if (normalization === 'origin-engines-v1') return new URL('/engines/v1/', input.origin);
  input.pathname = `${input.pathname.replace(/\/+$/, '')}/`;
  return input;
}

export function joinEndpoint(base: URL, path: string): URL {
  return new URL(path, path.startsWith('/') ? base.origin : base);
}
