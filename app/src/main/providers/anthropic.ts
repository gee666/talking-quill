import type {
  Destination,
  ModelInfo,
  ProviderConfig,
  VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import type { JsonTransport } from './json-transport';
import {
  MAX_NATIVE_PAGES,
  boundedString,
  completionText,
  freezeModels,
  modelInfo,
  optionalRecord,
  parseConfig,
  parseRequest,
  record,
  requireCredential,
  requireModel,
  staticVision,
  validation,
} from './native-common';
import { ProviderError } from './errors';

const PRODUCTION_BASE = 'https://api.anthropic.com/';
const VISION_MODELS = [/^claude-3(?:[.-]|$)/i, /^claude-(?:sonnet|opus|haiku)-[4-9](?:[.-]|$)/i];

export class AnthropicProvider implements SmartProvider {
  readonly id = 'anthropic' as const;
  readonly credentialPolicy = 'required' as const;
  readonly #transport: JsonTransport;
  readonly #override: string | undefined;

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
    let afterId: string | null = null;
    for (let page = 0; page < MAX_NATIVE_PAGES; page += 1) {
      const url = new URL('v1/models', this.#base());
      url.searchParams.set('limit', '100');
      if (afterId !== null) url.searchParams.set('after_id', afterId);
      const response = await this.#transport.request({
        url,
        method: 'GET',
        kind: 'model-list',
        headers: this.#headers(requireCredential(invocation)),
        credentialed: true,
        fixedCloud: this.#fixedCloud(),
        signal,
        maxResponseBytes: 2 * 1024 * 1024,
      });
      const body = record(response.body);
      if (!Array.isArray(body.data) || body.data.length > 1_000)
        throw new ProviderError('INVALID_RESPONSE');
      for (const item of body.data) {
        const model = record(item);
        const id = boundedString(model.id);
        if (model.type !== undefined && model.type !== 'model') continue;
        const name =
          typeof model.display_name === 'string' ? boundedString(model.display_name) : id;
        models.push(modelInfo(id, name, null, this.capabilities(invocation.config, id)));
      }
      if (body.has_more !== true) return freezeModels(models);
      const lastId = optionalRecord(body.data.at(-1))?.id;
      afterId = boundedString(lastId);
    }
    throw new ProviderError('INVALID_RESPONSE');
  }

  capabilities(_config: ProviderConfig, modelId: string): VisionCapability {
    return staticVision(modelId, VISION_MODELS);
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
    const credential = requireCredential(invocation);
    const request = parseRequest(requestInput);
    const model = requireModel(invocation.config, request.modelId);
    const response = await this.#transport.request({
      url: new URL('v1/messages', this.#base()),
      method: 'POST',
      kind: 'completion',
      headers: this.#headers(credential),
      credentialed: true,
      fixedCloud: this.#fixedCloud(),
      signal,
      body: {
        model,
        max_tokens: request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 1_024,
        temperature: request.temperature,
        messages: [
          {
            role: 'user',
            content:
              request.image === undefined
                ? request.input
                : [
                    { type: 'text', text: request.input },
                    {
                      type: 'image',
                      source: {
                        type: 'base64',
                        media_type: request.image.mimeType,
                        data: request.image.base64,
                      },
                    },
                  ],
          },
        ],
      },
      maxResponseBytes: 2 * 1024 * 1024,
    });
    const body = record(response.body);
    if (
      body.type !== 'message' ||
      body.role !== 'assistant' ||
      body.stop_reason !== 'end_turn' ||
      !Array.isArray(body.content)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const parts = body.content.map((item) => {
      const content = record(item);
      if (content.type !== 'text') throw new ProviderError('INVALID_RESPONSE');
      return typeof content.text === 'string' ? content.text : null;
    });
    return Object.freeze({ text: completionText(parts), destination: response.destination });
  }

  #validate(invocation: ProviderInvocationConfig): void {
    parseConfig(invocation.config, this.id);
    requireCredential(invocation);
  }

  #headers(credential: string): Readonly<Record<string, string>> {
    return Object.freeze({ 'x-api-key': credential, 'anthropic-version': '2023-06-01' });
  }

  #base(): URL {
    return new URL(this.#override ?? PRODUCTION_BASE);
  }
  #fixedCloud(): boolean {
    return this.#override === undefined;
  }
}
