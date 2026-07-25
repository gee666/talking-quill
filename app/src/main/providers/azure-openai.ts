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
  completionText,
  modelInfo,
  parseConfig,
  parseRequest,
  record,
  requireCredential,
  requireModel,
  validation,
} from './native-common';

const API_VERSION = '2024-10-21';
const AZURE_HOST_SUFFIXES = ['.openai.azure.com', '.openai.azure.us', '.openai.azure.cn'];

export class AzureOpenAIProvider implements SmartProvider {
  readonly id = 'azure' as const;
  readonly credentialPolicy = 'required' as const;
  readonly #transport: JsonTransport;
  readonly #override: string | undefined;

  constructor(transport: JsonTransport, endpointOverride?: string) {
    this.#transport = transport;
    this.#override = endpointOverride;
  }

  credentialBinding(config: ProviderConfig): string {
    return this.#configuredBase(config).href;
  }

  async validate(invocation: ProviderInvocationConfig, signal: AbortSignal) {
    this.#validate(invocation);
    const models = await this.listModels(invocation, signal);
    const completion = await this.#complete(
      invocation,
      { input: 'Reply with OK.', temperature: 0, maxOutputTokens: 8 },
      signal,
    );
    return validation(completion.destination, models.length);
  }

  listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    void signal;
    this.#validate(invocation);
    const model = requireModel(invocation.config);
    return Promise.resolve(
      Object.freeze([
        modelInfo(
          model,
          model,
          invocation.config.contextWindow ?? null,
          this.capabilities(invocation.config, model),
        ),
      ]),
    );
  }

  capabilities(config: ProviderConfig, modelId: string): VisionCapability {
    void config;
    void modelId;
    return 'unknown';
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
      this.#requestBase(invocation.config),
      { credentialed: true },
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
    const deployment = requireModel(invocation.config, request.modelId);
    const url = new URL(
      `openai/deployments/${encodeURIComponent(deployment)}/chat/completions`,
      this.#requestBase(invocation.config),
    );
    url.searchParams.set('api-version', API_VERSION);
    const reasoning = invocation.config.modelType === 'reasoning';
    const maxTokens = request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 1_024;
    const response = await this.#transport.request({
      url,
      method: 'POST',
      kind: 'completion',
      headers: { 'api-key': requireCredential(invocation) },
      credentialed: true,
      signal,
      body: {
        model: deployment,
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
        ...(reasoning
          ? { max_completion_tokens: maxTokens }
          : { temperature: request.temperature, max_tokens: maxTokens }),
      },
      maxResponseBytes: 2 * 1024 * 1024,
    });
    const body = record(response.body);
    if (!Array.isArray(body.choices) || body.choices.length !== 1)
      throw new ProviderError('INVALID_RESPONSE');
    const choice = record(body.choices[0]);
    if (choice.finish_reason !== 'stop') throw new ProviderError('INVALID_RESPONSE');
    const message = record(choice.message);
    if (message.role !== undefined && message.role !== 'assistant')
      throw new ProviderError('INVALID_RESPONSE');
    if (typeof message.content !== 'string') throw new ProviderError('INVALID_RESPONSE');
    return Object.freeze({
      text: completionText([message.content]),
      destination: response.destination,
    });
  }

  #validate(invocation: ProviderInvocationConfig): void {
    this.#configuredBase(parseConfig(invocation.config, this.id));
    requireCredential(invocation);
    requireModel(invocation.config);
  }

  #configuredBase(config: ProviderConfig): URL {
    if (config.baseUrl === undefined) throw new ProviderError('INVALID_CONFIG');
    const url = new URL(config.baseUrl);
    if (
      url.protocol !== 'https:' ||
      url.port.length > 0 ||
      url.pathname !== '/' ||
      url.search.length > 0 ||
      url.hash.length > 0 ||
      !AZURE_HOST_SUFFIXES.some((suffix) => url.hostname.endsWith(suffix))
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    return url;
  }

  #requestBase(config: ProviderConfig): URL {
    return new URL(this.#override ?? this.#configuredBase(config));
  }
}
