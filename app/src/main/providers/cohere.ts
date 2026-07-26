import type {
  Destination,
  ModelInfo,
  ProviderConfig,
  VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import { ProviderError } from './errors';
import type { JsonTransport } from './json-transport';
import {
  MAX_NATIVE_PAGES,
  boundedString,
  completionText,
  credentialFingerprint,
  freezeModels,
  modelInfo,
  parseConfig,
  parseRequest,
  record,
  requireCredential,
  requireModel,
  validation,
} from './native-common';

const PRODUCTION_BASE = 'https://api.cohere.com/';

export class CohereProvider implements SmartProvider {
  readonly id = 'cohere' as const;
  readonly credentialPolicy = 'required' as const;
  readonly #transport: JsonTransport;
  readonly #override: string | undefined;
  readonly #capabilities = new Map<string, VisionCapability>();
  readonly #preflightCapabilities = new Map<string, VisionCapability>();

  constructor(transport: JsonTransport, endpointOverride?: string) {
    this.#transport = transport;
    this.#override = endpointOverride;
  }
  credentialBinding(config: ProviderConfig): string {
    parseConfig(config, this.id);
    return PRODUCTION_BASE;
  }

  async validate(invocation: ProviderInvocationConfig, signal: AbortSignal) {
    this.#validate(invocation);
    const models = await this.listModels(invocation, signal);
    const model = requireModel(invocation.config);
    if (!models.some((item) => item.id === model)) throw new ProviderError('MODEL_NOT_FOUND');
    const completion = await this.#complete(
      invocation,
      { input: 'Reply with OK.', modelId: model, temperature: 0, maxOutputTokens: 8 },
      signal,
    );
    return validation(completion.destination, models.length);
  }

  async listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    this.#validate(invocation);
    const models: ModelInfo[] = [];
    let pageToken: string | null = null;
    for (let page = 0; page < MAX_NATIVE_PAGES; page += 1) {
      const url = new URL('v1/models', this.#base());
      url.searchParams.set('page_size', '1000');
      url.searchParams.set('endpoint', 'chat');
      if (pageToken !== null) url.searchParams.set('page_token', pageToken);
      const response = await this.#transport.request({
        url,
        method: 'GET',
        kind: 'model-list',
        headers: this.#headers(requireCredential(invocation)),
        credentialed: true,
        fixedCloud: this.#fixedCloud(),
        signal,
        maxResponseBytes: 4 * 1024 * 1024,
      });
      const body = record(response.body);
      if (!Array.isArray(body.models) || body.models.length > 2_000)
        throw new ProviderError('INVALID_RESPONSE');
      for (const item of body.models) {
        const candidate = record(item);
        const id = boundedString(candidate.name);
        const features = stringArray(candidate.features);
        const endpoints = stringArray(candidate.endpoints);
        if (endpoints.length > 0 && !endpoints.includes('chat')) continue;
        const vision: VisionCapability = features.includes('vision')
          ? 'supported'
          : features.length > 0
            ? 'unsupported'
            : 'unknown';
        const context =
          Number.isInteger(candidate.context_length) && Number(candidate.context_length) > 0
            ? Number(candidate.context_length)
            : null;
        models.push(modelInfo(id, id, context, vision));
      }
      if (body.next_page_token === undefined || body.next_page_token === null) {
        const catalog = freezeModels(models);
        this.#replaceCapabilities(catalog, credentialFingerprint(invocation));
        return catalog;
      }
      pageToken = boundedString(body.next_page_token, 2_048);
    }
    throw new ProviderError('INVALID_RESPONSE');
  }

  capabilities(_config: ProviderConfig, modelId: string): VisionCapability {
    return this.#capabilities.get(modelId) ?? 'unknown';
  }

  async capabilityPreflight(
    invocation: ProviderInvocationConfig,
    modelId: string,
    signal: AbortSignal,
  ): Promise<VisionCapability> {
    this.#validate(invocation);
    const requestedModel = requireModel(invocation.config, modelId);
    const credentialScope = credentialFingerprint(invocation);
    const cached = this.#preflightCapabilities.get(
      this.#preflightKey(credentialScope, requestedModel),
    );
    if (cached !== undefined) return cached;
    const models = await this.listModels(invocation, signal);
    return models.find((model) => model.id === requestedModel)?.vision ?? 'unknown';
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    request: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
  ): Promise<string> {
    return (await this.#complete(invocation, request, signal)).text;
  }
  classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination> {
    this.#validate(invocation);
    return this.#transport.classify(
      this.#base(),
      { credentialed: true, fixedCloud: this.#fixedCloud() },
      signal,
    );
  }

  async #complete(
    invocation: ProviderInvocationConfig,
    requestInput: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
  ) {
    this.#validate(invocation);
    const request = parseRequest(requestInput);
    const model = requireModel(invocation.config, request.modelId);
    const response = await this.#transport.request({
      url: new URL('v2/chat', this.#base()),
      method: 'POST',
      kind: 'completion',
      headers: this.#headers(requireCredential(invocation)),
      credentialed: true,
      fixedCloud: this.#fixedCloud(),
      signal,
      body: {
        model,
        messages: [
          {
            role: 'user',
            content:
              request.image === undefined
                ? request.input
                : [
                    { type: 'text', text: request.input },
                    {
                      type: 'image_url',
                      image_url: {
                        url: `data:${request.image.mimeType};base64,${request.image.base64}`,
                      },
                    },
                  ],
          },
        ],
        stream: false,
        temperature: request.temperature,
        max_tokens: request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 1_024,
      },
      maxResponseBytes: 2 * 1024 * 1024,
    });
    const body = record(response.body);
    if (body.finish_reason !== 'COMPLETE') throw new ProviderError('INVALID_RESPONSE');
    const message = record(body.message);
    if (message.role !== 'assistant' || !Array.isArray(message.content))
      throw new ProviderError('INVALID_RESPONSE');
    const parts = message.content.map((item) => {
      const content = record(item);
      if (content.type !== 'text' || typeof content.text !== 'string')
        throw new ProviderError('INVALID_RESPONSE');
      return content.text;
    });
    return Object.freeze({ text: completionText(parts), destination: response.destination });
  }

  #replaceCapabilities(models: readonly ModelInfo[], credentialScope: string): void {
    this.#capabilities.clear();
    this.#preflightCapabilities.clear();
    for (const model of models) {
      this.#capabilities.set(model.id, model.vision);
      this.#preflightCapabilities.set(this.#preflightKey(credentialScope, model.id), model.vision);
    }
  }

  #preflightKey(credentialScope: string, modelId: string): string {
    return `${credentialScope}\n${modelId}`;
  }

  #validate(invocation: ProviderInvocationConfig): void {
    parseConfig(invocation.config, this.id);
    requireCredential(invocation);
  }
  #headers(credential: string): Readonly<Record<string, string>> {
    return Object.freeze({ authorization: `Bearer ${credential}` });
  }
  #base(): URL {
    return new URL(this.#override ?? PRODUCTION_BASE);
  }
  #fixedCloud(): boolean {
    return this.#override === undefined;
  }
}

function stringArray(input: unknown): readonly string[] {
  if (!Array.isArray(input) || input.length > 64) return [];
  return input.filter((item): item is string => typeof item === 'string' && item.length <= 128);
}
