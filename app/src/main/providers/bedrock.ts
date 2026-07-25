import type {
  Destination,
  ModelInfo,
  ProviderConfig,
  VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig, SmartProvider } from './contracts';
import { ProviderError } from './errors';
import { parseAwsCredentials, signAwsRequest } from './aws-sigv4';
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
  requireModel,
  validation,
} from './native-common';

export class BedrockProvider implements SmartProvider {
  readonly id = 'bedrock' as const;
  readonly credentialPolicy = 'required' as const;
  readonly #transport: JsonTransport;
  readonly #override: string | undefined;
  readonly #capabilities = new Map<string, VisionCapability>();

  constructor(transport: JsonTransport, endpointOverride?: string) {
    this.#transport = transport;
    this.#override = endpointOverride;
  }

  credentialBinding(config: ProviderConfig): string {
    const parsed = parseConfig(config, this.id);
    return `aws-sigv4:${requiredRegion(parsed)}`;
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
    const models = await this.#listFoundationModels(invocation, signal);
    models.push(...(await this.#listInferenceProfiles(invocation, signal)));
    return freezeModels(models);
  }

  capabilities(config: ProviderConfig, modelId: string): VisionCapability {
    return this.#capabilities.get(this.#key(config, modelId)) ?? 'unknown';
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
      this.#runtimeBase(invocation.config),
      { credentialed: true, fixedCloud: this.#fixedCloud() },
      signal,
    );
  }

  async #listFoundationModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ModelInfo[]> {
    const models: ModelInfo[] = [];
    let nextToken: string | null = null;
    for (let page = 0; page < MAX_NATIVE_PAGES; page += 1) {
      const url = new URL('foundation-models', this.#controlBase(invocation.config));
      url.searchParams.set('byOutputModality', 'TEXT');
      if (nextToken !== null) url.searchParams.set('nextToken', nextToken);
      const body = record((await this.#modelListRequest(invocation, url, signal)).body);
      if (!Array.isArray(body.modelSummaries) || body.modelSummaries.length > 2_000)
        throw new ProviderError('INVALID_RESPONSE');
      for (const item of body.modelSummaries) {
        const summary = record(item);
        const output = stringArray(summary.outputModalities);
        const inference = stringArray(summary.inferenceTypesSupported);
        if (!output.includes('TEXT') || (inference.length > 0 && !inference.includes('ON_DEMAND')))
          continue;
        const id = boundedString(summary.modelId);
        const name = typeof summary.modelName === 'string' ? boundedString(summary.modelName) : id;
        const input = stringArray(summary.inputModalities);
        const vision: VisionCapability = input.includes('IMAGE')
          ? 'supported'
          : input.length > 0
            ? 'unsupported'
            : 'unknown';
        this.#capabilities.set(this.#key(invocation.config, id), vision);
        models.push(modelInfo(id, name, invocation.config.contextWindow ?? null, vision));
      }
      if (body.nextToken === undefined || body.nextToken === null) return models;
      nextToken = boundedString(body.nextToken, 2_048);
    }
    throw new ProviderError('INVALID_RESPONSE');
  }

  async #listInferenceProfiles(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ModelInfo[]> {
    const profiles: ModelInfo[] = [];
    let nextToken: string | null = null;
    for (let page = 0; page < MAX_NATIVE_PAGES; page += 1) {
      const url = new URL('inference-profiles', this.#controlBase(invocation.config));
      if (nextToken !== null) url.searchParams.set('nextToken', nextToken);
      const body = record((await this.#modelListRequest(invocation, url, signal)).body);
      if (
        !Array.isArray(body.inferenceProfileSummaries) ||
        body.inferenceProfileSummaries.length > 2_000
      )
        throw new ProviderError('INVALID_RESPONSE');
      for (const item of body.inferenceProfileSummaries) {
        const summary = record(item);
        if (summary.status !== undefined && summary.status !== 'ACTIVE') continue;
        const id = boundedString(summary.inferenceProfileId);
        const name =
          typeof summary.inferenceProfileName === 'string'
            ? boundedString(summary.inferenceProfileName)
            : id;
        const vision = this.#profileVision(invocation.config, summary.models);
        this.#capabilities.set(this.#key(invocation.config, id), vision);
        profiles.push(modelInfo(id, name, invocation.config.contextWindow ?? null, vision));
        const arn = readInferenceProfileArn(summary.inferenceProfileArn);
        if (arn !== null && arn !== id) {
          this.#capabilities.set(this.#key(invocation.config, arn), vision);
          const aliasName = `${name} (ARN)`;
          profiles.push(
            modelInfo(
              arn,
              aliasName.length <= 512 ? aliasName : arn,
              invocation.config.contextWindow ?? null,
              vision,
            ),
          );
        }
      }
      if (body.nextToken === undefined || body.nextToken === null) return profiles;
      nextToken = boundedString(body.nextToken, 2_048);
    }
    throw new ProviderError('INVALID_RESPONSE');
  }

  #profileVision(config: ProviderConfig, input: unknown): VisionCapability {
    if (!Array.isArray(input) || input.length === 0 || input.length > 64) return 'unknown';
    const capabilities = input.map((item) => {
      const arn = boundedString(record(item).modelArn, 2_048);
      const marker = 'foundation-model/';
      const index = arn.indexOf(marker);
      if (index < 0) return 'unknown';
      return this.capabilities(config, arn.slice(index + marker.length));
    });
    if (capabilities.some((capability) => capability === 'supported')) return 'supported';
    if (capabilities.every((capability) => capability === 'unsupported')) return 'unsupported';
    return 'unknown';
  }

  #modelListRequest(invocation: ProviderInvocationConfig, url: URL, signal: AbortSignal) {
    return this.#transport.request({
      url,
      method: 'GET',
      kind: 'model-list',
      headers: this.#signedHeaders(invocation, url, 'GET'),
      credentialed: true,
      fixedCloud: this.#fixedCloud(),
      signal,
      maxResponseBytes: 4 * 1024 * 1024,
    });
  }

  async #complete(
    invocation: ProviderInvocationConfig,
    requestInput: Parameters<SmartProvider['cleanTranscript']>[1],
    signal: AbortSignal,
  ) {
    this.#validate(invocation);
    const request = parseRequest(requestInput);
    const model = requireModel(invocation.config, request.modelId);
    const url = new URL(
      `model/${encodeURIComponent(model)}/converse`,
      this.#runtimeBase(invocation.config),
    );
    const body = {
      messages: [
        {
          role: 'user',
          content: [
            { text: request.input },
            ...(request.image === undefined
              ? []
              : [
                  {
                    image: {
                      format: 'jpeg',
                      source: { bytes: request.image.base64 },
                    },
                  },
                ]),
          ],
        },
      ],
      inferenceConfig: {
        ...(supportsBedrockTemperature(model) ? { temperature: request.temperature } : {}),
        maxTokens: request.maxOutputTokens ?? invocation.config.maxOutputTokens ?? 1_024,
      },
    };
    const response = await this.#transport.request({
      url,
      method: 'POST',
      kind: 'completion',
      headers: this.#signedHeaders(invocation, url, 'POST', body),
      credentialed: true,
      fixedCloud: this.#fixedCloud(),
      signal,
      body,
      maxResponseBytes: 2 * 1024 * 1024,
    });
    const responseBody = record(response.body);
    if (responseBody.stopReason !== 'end_turn') throw new ProviderError('INVALID_RESPONSE');
    const output = record(responseBody.output);
    const message = record(output.message);
    if (message.role !== 'assistant' || !Array.isArray(message.content))
      throw new ProviderError('INVALID_RESPONSE');
    const parts = message.content.map((item) => {
      const content = record(item);
      if (typeof content.text !== 'string') throw new ProviderError('INVALID_RESPONSE');
      return content.text;
    });
    return Object.freeze({ text: completionText(parts), destination: response.destination });
  }

  #validate(invocation: ProviderInvocationConfig): void {
    const config = parseConfig(invocation.config, this.id);
    if (config.region === undefined) throw new ProviderError('INVALID_CONFIG');
    this.#credentials(invocation);
  }
  #credentials(invocation: ProviderInvocationConfig) {
    if (invocation.credential === null) throw new ProviderError('MISSING_CREDENTIAL');
    return parseAwsCredentials(invocation.credential);
  }
  #signedHeaders(
    invocation: ProviderInvocationConfig,
    url: URL,
    method: 'GET' | 'POST',
    body?: unknown,
  ): Readonly<Record<string, string>> {
    return signAwsRequest({
      method,
      url,
      ...(body === undefined ? {} : { body }),
      region: requiredRegion(invocation.config),
      service: 'bedrock',
      credentials: this.#credentials(invocation),
    });
  }
  #controlBase(config: ProviderConfig): URL {
    if (this.#override !== undefined) return new URL(this.#override);
    return new URL(`https://bedrock.${requiredRegion(config)}.amazonaws.com/`);
  }
  #runtimeBase(config: ProviderConfig): URL {
    if (this.#override !== undefined) return new URL(this.#override);
    return new URL(`https://bedrock-runtime.${requiredRegion(config)}.amazonaws.com/`);
  }
  #fixedCloud(): boolean {
    return this.#override === undefined;
  }
  #key(config: ProviderConfig, modelId: string): string {
    return `${config.region ?? ''}\n${modelId}`;
  }
}

function supportsBedrockTemperature(modelId: string): boolean {
  return ![
    'anthropic.claude-opus-4-7',
    'anthropic.claude-opus-4-8',
    'anthropic.claude-sonnet-5',
  ].some((model) => modelId.includes(model));
}

function readInferenceProfileArn(input: unknown): string | null {
  if (input === undefined || input === null) return null;
  const arn = boundedString(input);
  if (
    !/^arn:(?:aws|aws-us-gov|aws-cn):bedrock:[a-z0-9-]+:(?:\d{12})?:(?:application-)?inference-profile\/[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/.test(
      arn,
    )
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return arn;
}

function requiredRegion(config: ProviderConfig): string {
  if (config.region === undefined) throw new ProviderError('INVALID_CONFIG');
  return config.region;
}

function stringArray(input: unknown): readonly string[] {
  if (!Array.isArray(input) || input.length > 64) return [];
  return input.filter((item): item is string => typeof item === 'string' && item.length <= 128);
}
