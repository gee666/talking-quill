import {
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
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
import type { ModelFilter, ModelListFormat, OpenAICompatiblePreset } from './presets';
import { staticVisionCapability } from './vision-capabilities';

const MAX_MODELS = 10_000;
const MAX_RESPONSE_OUTPUT_ITEMS = 256;
const MAX_RESPONSE_CONTENT_ITEMS = 512;
const MAX_COMPLETION_CHARACTERS = 200_000;

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
      selectedModel !== undefined &&
      selectedModel !== null &&
      models.length > 0 &&
      !models.some((model) => model.id === selectedModel)
    ) {
      throw new ProviderError('MODEL_NOT_FOUND');
    }
    const probeModel = selectedModel;
    if (probeModel === null || probeModel === undefined) {
      throw new ProviderError('INVALID_CONFIG');
    }
    const probe = await this.#complete(
      invocation,
      {
        input: 'Reply with OK.',
        modelId: probeModel,
        temperature: 0,
        maxOutputTokens: 1,
      },
      signal,
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
  ): Promise<{ readonly text: string; readonly destination: Destination }> {
    this.#validateConfig(invocation.config);
    this.#validateCredential(invocation.credential, true);
    const request = parseCompletionRequest(requestInput);
    const model = request.modelId ?? invocation.config.modelId ?? this.#preset.defaultModel;
    if (model === null && this.id !== 'textgenwebui') {
      throw new ProviderError('INVALID_CONFIG');
    }
    const maxOutputTokens =
      request.maxOutputTokens ??
      invocation.config.maxOutputTokens ??
      this.#preset.defaultMaxOutputTokens;
    await this.#prepareCompletion(invocation, model, signal);
    const body =
      this.#preset.protocol === 'responses'
        ? createResponsesBody(this.#preset, model, request, maxOutputTokens)
        : createChatCompletionsBody(this.#preset, model, request, maxOutputTokens);
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
    const maximumCharacters = Math.min(
      MAX_COMPLETION_CHARACTERS,
      Math.max(16, maxOutputTokens * 16),
    );
    const text =
      this.#preset.protocol === 'responses'
        ? parseResponsesOutput(response.body, maximumCharacters)
        : parseChatCompletionOutput(response.body, maximumCharacters);
    if (text.trim().length === 0) throw new ProviderError('INVALID_RESPONSE');
    return Object.freeze({ text, destination: response.destination });
  }

  async #prepareCompletion(
    invocation: ProviderInvocationConfig,
    model: string | null | undefined,
    signal: AbortSignal,
  ): Promise<void> {
    if (this.#preset.preparation !== 'lemonade-load') return;
    if (model === null || model === undefined) throw new ProviderError('INVALID_CONFIG');
    const installedModels = await this.listModels(invocation, signal);
    if (!installedModels.some((installed) => installed.id === model)) {
      throw new ProviderError('MODEL_NOT_FOUND');
    }
    const baseUrl = this.#baseUrl(invocation.config);
    const headers = this.#authHeaders(invocation.credential);
    const health = await this.#transport.request({
      url: joinEndpoint(baseUrl, 'health'),
      method: 'GET',
      kind: 'model-detail',
      headers,
      credentialed: invocation.credential !== null,
      fixedCloud: this.#isFixedCloud(),
      signal,
      ...(invocation.config.timeoutMs === undefined
        ? {}
        : { timeoutMs: invocation.config.timeoutMs }),
      maxResponseBytes: 512 * 1024,
    });
    const healthRecord = asRecord(health.body);
    const loaded = healthRecord?.all_models_loaded;
    if (loaded !== undefined && !Array.isArray(loaded)) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const desiredContext = invocation.config.contextWindow ?? this.#preset.defaultContextWindow;
    if (
      Array.isArray(loaded) &&
      loaded.slice(0, 256).some((item) => {
        const record = asRecord(item);
        const recipe = asRecord(record?.recipe_options);
        return (
          readString(record?.model_name) === model &&
          readPositiveInteger(recipe?.ctx_size) === desiredContext
        );
      })
    ) {
      return;
    }
    const load = await this.#transport.request({
      url: joinEndpoint(baseUrl, 'load'),
      method: 'POST',
      kind: 'model-detail',
      headers,
      body: Object.freeze({ model_name: model, ctx_size: desiredContext }),
      credentialed: invocation.credential !== null,
      fixedCloud: this.#isFixedCloud(),
      signal,
      ...(invocation.config.timeoutMs === undefined
        ? {}
        : { timeoutMs: invocation.config.timeoutMs }),
      maxResponseBytes: 512 * 1024,
    });
    if (asRecord(load.body)?.status !== 'success') {
      throw new ProviderError('INVALID_RESPONSE');
    }
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

function parseCompletionRequest(
  input: unknown,
): ReturnType<typeof ProviderCompletionRequestSchema.parse> {
  try {
    return ProviderCompletionRequestSchema.parse(input);
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

function createResponsesBody(
  preset: OpenAICompatiblePreset,
  model: string | null | undefined,
  request: ReturnType<typeof ProviderCompletionRequestSchema.parse>,
  maxOutputTokens: number,
): Readonly<Record<string, unknown>> {
  const temperature =
    preset.temperatureMode === 'openai-reasoning-one' &&
    model !== null &&
    model !== undefined &&
    (/^o\d(?:[.-]|$)/i.test(model) || /^gpt-5(?:[.-]|$)/i.test(model))
      ? 1
      : request.temperature;
  return Object.freeze({
    model,
    input:
      request.image === undefined
        ? request.input
        : Object.freeze([
            Object.freeze({
              role: 'user',
              content: Object.freeze([
                Object.freeze({ type: 'input_text', text: request.input }),
                Object.freeze({ type: 'input_image', image_url: imageDataUrl(request.image) }),
              ]),
            }),
          ]),
    store: false,
    max_output_tokens: maxOutputTokens,
    temperature,
  });
}

function createChatCompletionsBody(
  preset: OpenAICompatiblePreset,
  model: string | null | undefined,
  request: ReturnType<typeof ProviderCompletionRequestSchema.parse>,
  maxOutputTokens: number,
): Readonly<Record<string, unknown>> {
  const maxTokensField = preset.maxTokensField ?? 'max_tokens';
  return Object.freeze({
    ...(model === null || model === undefined ? {} : { model }),
    messages: Object.freeze([
      Object.freeze({
        role: 'user',
        content:
          request.image === undefined
            ? request.input
            : Object.freeze([
                Object.freeze({ type: 'text', text: request.input }),
                Object.freeze({
                  type: 'image_url',
                  image_url: Object.freeze({ url: imageDataUrl(request.image) }),
                }),
              ]),
      }),
    ]),
    stream: false,
    temperature: request.temperature,
    [maxTokensField]: maxOutputTokens,
  });
}

function imageDataUrl(image: { readonly mimeType: 'image/jpeg'; readonly base64: string }): string {
  return `data:${image.mimeType};base64,${image.base64}`;
}

function parseResponsesOutput(input: unknown, maximumCharacters: number): string {
  const record = asRecord(input);
  if (record === null) throw new ProviderError('INVALID_RESPONSE');
  if (record.status !== 'completed') throw new ProviderError('INVALID_RESPONSE');
  if (record.error !== undefined && record.error !== null) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (!Array.isArray(record.output) || record.output.length > MAX_RESPONSE_OUTPUT_ITEMS) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const parts: string[] = [];
  let characters = 0;
  for (const item of record.output) {
    const output = asRecord(item);
    if (output === null) throw new ProviderError('INVALID_RESPONSE');
    if (output.type !== 'message') continue;
    if (output.role !== 'assistant') throw new ProviderError('INVALID_RESPONSE');
    if (!Array.isArray(output.content) || output.content.length > MAX_RESPONSE_CONTENT_ITEMS) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    for (const contentItem of output.content) {
      const content = asRecord(contentItem);
      if (content === null || typeof content.type !== 'string') {
        throw new ProviderError('INVALID_RESPONSE');
      }
      if (content.type === 'output_text') {
        if (typeof content.text !== 'string') throw new ProviderError('INVALID_RESPONSE');
        characters += content.text.length;
        if (characters > maximumCharacters) throw new ProviderError('INVALID_RESPONSE');
        parts.push(content.text);
      } else {
        // Refusals and other message content must not be mixed into transcript output.
        throw new ProviderError('INVALID_RESPONSE');
      }
    }
  }
  if (parts.length === 0) throw new ProviderError('INVALID_RESPONSE');
  return parts.join('');
}

function parseChatCompletionOutput(input: unknown, maximumCharacters: number): string {
  const record = asRecord(input);
  const choices = record?.choices;
  if (!Array.isArray(choices) || choices.length === 0) throw new ProviderError('INVALID_RESPONSE');
  const first = asRecord(choices[0]);
  const message = asRecord(first?.message);
  if (typeof message?.content !== 'string') throw new ProviderError('INVALID_RESPONSE');
  return boundedOutput(message.content, maximumCharacters);
}

function boundedOutput(value: string, maximumCharacters: number): string {
  if (value.length > maximumCharacters) throw new ProviderError('INVALID_RESPONSE');
  return value;
}

function normalizeModels(
  input: unknown,
  format: ModelListFormat,
  filter: ModelFilter,
  fallbackContext: number,
  contextWindows: Readonly<Record<string, number>>,
  fallbackVision: VisionCapability,
): readonly ModelInfo[] {
  const candidates = modelCandidates(input, format);
  if (candidates.length > MAX_MODELS) throw new ProviderError('INVALID_RESPONSE');
  const seen = new Set<string>();
  const models: ModelInfo[] = [];
  for (const candidate of candidates) {
    const record = asRecord(candidate);
    if (record === null || !passesModelFilter(record, filter, format)) continue;
    const id = modelId(record, format);
    if (id === null || seen.has(id) || hasControlCharacters(id)) continue;
    seen.add(id);
    const name = modelName(record, id, format);
    const loadedInstance = Array.isArray(record.loaded_instances)
      ? asRecord(record.loaded_instances[0])
      : null;
    const loadedConfig = asRecord(loadedInstance?.config);
    const contextWindow = readPositiveInteger(
      record.context_length ??
        loadedConfig?.context_length ??
        record.context_size ??
        record.max_model_len ??
        record.max_context_length ??
        record.max_tokens ??
        record.maxLength ??
        asRecord(record.limits)?.max_context_length ??
        readGgufContext(record) ??
        addPositiveIntegers(record.maxInputTokens, record.maxOutputTokens),
    );
    models.push(
      ModelInfoSchema.parse({
        id,
        name,
        contextWindow: contextWindow ?? contextWindows[id] ?? fallbackContext,
        vision: inferVision(record, fallbackVision),
      }),
    );
  }
  return Object.freeze(models);
}

function modelCandidates(input: unknown, format: ModelListFormat): readonly unknown[] {
  const candidates = Array.isArray(input) ? input : candidatesFromRecord(input, format);
  if (format !== 'docker') return candidates;
  const models: Readonly<Record<string, unknown>>[] = [];
  for (const candidate of candidates) {
    const record = asRecord(candidate);
    if (record === null || !Array.isArray(record.tags)) continue;
    for (const tag of record.tags.slice(0, MAX_MODELS)) {
      if (typeof tag !== 'string') continue;
      models.push(Object.freeze({ ...record, id: tag, name: tag }));
    }
  }
  return models;
}

function candidatesFromRecord(input: unknown, format: ModelListFormat): readonly unknown[] {
  const record = asRecord(input);
  if (record === null) throw new ProviderError('INVALID_RESPONSE');
  if (Array.isArray(record.data)) return record.data;
  if (Array.isArray(record.models)) return record.models;
  if (format === 'apipie' && Array.isArray(record.items)) return record.items;
  throw new ProviderError('INVALID_RESPONSE');
}

function modelId(
  record: Readonly<Record<string, unknown>>,
  format: ModelListFormat,
): string | null {
  if (format === 'apipie') {
    const provider = readString(record.provider);
    const model = readString(record.model ?? record.id);
    if (provider !== null && model !== null && !model.includes('/')) return `${provider}/${model}`;
  }
  return readString(record.id ?? record.key ?? record.name ?? record.model_name ?? record.model);
}

function modelName(
  record: Readonly<Record<string, unknown>>,
  id: string,
  format: ModelListFormat,
): string {
  if (format === 'docker') return id.includes('/') ? (id.split('/').at(-1) ?? id) : id;
  if (format === 'lemonade') {
    const organization = /^[a-z]+/i.exec(id)?.[0];
    return organization === undefined ? id : `${organization}:${id}`;
  }
  if (format === 'privatemode') {
    return id
      .split('-')
      .map((word) => `${word.charAt(0).toUpperCase()}${word.slice(1)}`)
      .join(' ');
  }
  return readString(record.display_name ?? record.title ?? record.name ?? record.label) ?? id;
}

function passesModelFilter(
  record: Readonly<Record<string, unknown>>,
  filter: ModelFilter,
  format: ModelListFormat,
): boolean {
  const id = (readString(record.id ?? record.name ?? record.model) ?? '').toLowerCase();
  if (filter === 'openai') {
    const owner = (readString(record.owned_by) ?? '').toLowerCase();
    const custom = owner.length > 0 && owner !== 'system' && !owner.includes('openai');
    return (
      custom ||
      ((id.includes('gpt') || id.startsWith('o')) &&
        !/(?:audio|realtime|image|moderation|transcri|instruct|vision)/.test(id) &&
        !id.startsWith('ft:'))
    );
  }
  if (filter === 'groq') return !id.includes('whisper') && !id.includes('tool-use');
  if (filter === 'chat') {
    if (format === 'together') return readString(record.type)?.toLowerCase() === 'chat';
    const subtype = readString(record.subtype)?.toLowerCase() ?? '';
    return subtype.includes('chat') || subtype.includes('chatx');
  }
  if (filter === 'context-required') return readPositiveInteger(record.context_length) !== null;
  if (filter === 'no-embed') {
    const type = readString(record.type)?.toLowerCase() ?? '';
    return type !== 'embeddings' && !id.includes('embed') && !/(?:^|[/:-])all-mini/i.test(id);
  }
  if (filter === 'no-whisper') return !id.startsWith('whisper');
  if (filter === 'lemonade-llm') {
    const labels = readStringArray(record.labels).map((label) => label.toLowerCase());
    return !labels.includes('embeddings') && !labels.includes('reranking');
  }
  if (filter === 'cometapi') {
    return !/(?:dall-?e|midjourney|mj_|stable-diffusion|sd-|flux-|playground-v|ideogram|recraft|black-forest-labs|stability-ai|sdxl|suno_|tts|whisper|runway|luma[_-]|veo|kling_|minimax_video|hunyuan-t1|embedding|search-gpts|files_retrieve|moderation|deepl)/.test(
      id,
    );
  }
  if (filter === 'privatemode-generate') {
    return !id.includes('/') && readStringArray(record.tasks).includes('generate');
  }
  return true;
}

function inferVision(
  record: Readonly<Record<string, unknown>>,
  fallback: VisionCapability,
): VisionCapability {
  const capabilityRecord = asRecord(record.capabilities);
  if (capabilityRecord?.vision === true) return 'supported';
  if (capabilityRecord?.vision === false) return 'unsupported';
  const values = [
    ...readStringArray(record.capabilities),
    ...readStringArray(record.modalities),
    ...readStringArray(record.labels),
  ];
  if (values.some((value) => /vision|image/i.test(value))) return 'supported';
  return fallback;
}

function asRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}

function hasControlCharacters(value: string): boolean {
  for (const character of value) {
    const code = character.charCodeAt(0);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

function readString(input: unknown): string | null {
  return typeof input === 'string' && input.trim().length > 0 && input.length <= 512
    ? input.trim()
    : null;
}

function readStringArray(input: unknown): readonly string[] {
  return Array.isArray(input)
    ? input.filter(
        (value): value is string =>
          typeof value === 'string' && value.length > 0 && value.length <= 128,
      )
    : [];
}

function readGgufContext(record: Readonly<Record<string, unknown>>): number | null {
  const config = asRecord(record.config);
  const gguf = asRecord(config?.gguf);
  if (gguf === null) return null;
  for (const [key, value] of Object.entries(gguf)) {
    if (key.endsWith('.context_length')) return readPositiveInteger(value);
  }
  return null;
}

function readPositiveInteger(input: unknown): number | null {
  return typeof input === 'number' && Number.isInteger(input) && input > 0 && input <= 2_000_000
    ? input
    : null;
}

function addPositiveIntegers(left: unknown, right: unknown): number | null {
  const first = readPositiveInteger(left);
  const second = readPositiveInteger(right);
  return first === null || second === null ? null : first + second;
}
