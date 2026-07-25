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
  freezeModels,
  modelInfo,
  parseConfig,
  parseRequest,
  record,
  requireCredential,
  requireModel,
  validation,
} from './native-common';

const PRODUCTION_BASE = 'https://generativelanguage.googleapis.com/';
export class GeminiProvider implements SmartProvider {
  readonly id = 'gemini' as const;
  readonly credentialPolicy = 'required' as const;
  readonly #transport: JsonTransport;
  readonly #override: string | undefined;
  readonly #vision = new Map<string, VisionCapability>();

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
      const url = new URL('v1beta/models', this.#base());
      url.searchParams.set('pageSize', '1000');
      if (pageToken !== null) url.searchParams.set('pageToken', pageToken);
      const response = await this.#transport.request({
        url,
        method: 'GET',
        kind: 'model-list',
        headers: this.#headers(requireCredential(invocation)),
        credentialed: true,
        fixedCloud: this.#fixedCloud(),
        signal,
        maxResponseBytes: 4 * 1024 * 1024,
        errorResponsePolicy: 'gemini-api-key',
      });
      const body = record(response.body);
      if (!Array.isArray(body.models) || body.models.length > 2_000)
        throw new ProviderError('INVALID_RESPONSE');
      for (const item of body.models) {
        const candidate = record(item);
        if (
          !Array.isArray(candidate.supportedGenerationMethods) ||
          !candidate.supportedGenerationMethods.includes('generateContent')
        )
          continue;
        const resource = boundedString(candidate.name);
        const id = resource.startsWith('models/') ? resource.slice(7) : resource;
        const name =
          typeof candidate.displayName === 'string' ? boundedString(candidate.displayName) : id;
        const context =
          Number.isInteger(candidate.inputTokenLimit) && Number(candidate.inputTokenLimit) > 0
            ? Number(candidate.inputTokenLimit)
            : null;
        const vision = inferGeminiVision(candidate, id);
        this.#vision.set(id, vision);
        models.push(modelInfo(id, name, context, vision));
      }
      if (body.nextPageToken === undefined || body.nextPageToken === null)
        return freezeModels(models);
      pageToken = boundedString(body.nextPageToken, 2_048);
    }
    throw new ProviderError('INVALID_RESPONSE');
  }

  capabilities(_config: ProviderConfig, modelId: string): VisionCapability {
    return this.#vision.get(modelId) ?? inferGeminiVision({}, modelId);
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
      url: new URL(`v1beta/models/${encodeURIComponent(model)}:generateContent`, this.#base()),
      method: 'POST',
      kind: 'completion',
      headers: this.#headers(requireCredential(invocation)),
      credentialed: true,
      fixedCloud: this.#fixedCloud(),
      signal,
      body: {
        contents: [
          {
            role: 'user',
            parts: [
              { text: request.input },
              ...(request.image === undefined
                ? []
                : [
                    {
                      inlineData: {
                        mimeType: request.image.mimeType,
                        data: request.image.base64,
                      },
                    },
                  ]),
            ],
          },
        ],
        generationConfig: {
          temperature: request.temperature,
          maxOutputTokens: request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 1_024,
        },
      },
      maxResponseBytes: 2 * 1024 * 1024,
      errorResponsePolicy: 'gemini-api-key',
    });
    const body = record(response.body);
    if (!Array.isArray(body.candidates) || body.candidates.length !== 1)
      throw new ProviderError('INVALID_RESPONSE');
    const candidate = record(body.candidates[0]);
    if (candidate.finishReason !== 'STOP') throw new ProviderError('INVALID_RESPONSE');
    const content = record(candidate.content);
    if (content.role !== undefined && content.role !== 'model')
      throw new ProviderError('INVALID_RESPONSE');
    if (!Array.isArray(content.parts)) throw new ProviderError('INVALID_RESPONSE');
    const parts = content.parts.map((item) => {
      const part = record(item);
      if (typeof part.text !== 'string') throw new ProviderError('INVALID_RESPONSE');
      return part.text;
    });
    return Object.freeze({ text: completionText(parts), destination: response.destination });
  }

  #validate(invocation: ProviderInvocationConfig): void {
    parseConfig(invocation.config, this.id);
    requireCredential(invocation);
  }
  #headers(credential: string): Readonly<Record<string, string>> {
    return Object.freeze({ 'x-goog-api-key': credential });
  }
  #base(): URL {
    return new URL(this.#override ?? PRODUCTION_BASE);
  }
  #fixedCloud(): boolean {
    return this.#override === undefined;
  }
}

function inferGeminiVision(
  metadata: Readonly<Record<string, unknown>>,
  modelId: string,
): VisionCapability {
  const modalities = [
    ...stringArray(metadata.supportedInputModalities),
    ...stringArray(metadata.inputModalities),
  ].map((value) => value.toUpperCase());
  if (modalities.includes('IMAGE')) return 'supported';
  if (modalities.length > 0) return 'unsupported';
  if (/^gemma-3-1b-it(?:[.-]|$)/i.test(modelId)) return 'unsupported';
  if (/^gemma-3-(?:4b|12b|27b)-it(?:[.-]|$)/i.test(modelId)) return 'supported';
  if (/^gemini-(?:pro$|1\.0-pro(?:[.-]|$))/i.test(modelId)) return 'unsupported';
  if (/^gemini-(?:1\.5|2(?:\.|-|$)|2\.5|3(?:\.|-|$)|pro-vision)/i.test(modelId)) {
    return 'supported';
  }
  return 'unknown';
}

function stringArray(input: unknown): readonly string[] {
  if (!Array.isArray(input) || input.length > 32) return [];
  return input.filter(
    (value): value is string => typeof value === 'string' && value.length > 0 && value.length <= 64,
  );
}
