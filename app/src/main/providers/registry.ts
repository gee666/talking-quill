import {
  PROVIDER_IDS,
  ProviderCatalogEntrySchema,
  type ProviderCatalogEntry,
  type ProviderId,
} from '../../shared/schemas/providers';
import type { SmartProvider } from './contracts';
import { PinnedJsonTransport, type JsonTransport } from './json-transport';
import { OllamaProvider } from './ollama';
import { createOpenAICompatibleProvider } from './openai-compatible';
import { OPENAI_COMPATIBLE_PRESETS } from './presets';
import { PiProvider, type PiProviderOptions } from './pi';
import { AnthropicProvider } from './anthropic';
import { AzureOpenAIProvider } from './azure-openai';
import { BedrockProvider } from './bedrock';
import { CohereProvider } from './cohere';
import { GeminiProvider } from './gemini';

export interface ProviderRegistryOptions {
  readonly transport?: JsonTransport;
  /** Test-only endpoint substitution. Production callers must leave this undefined. */
  readonly endpointOverrides?: Readonly<Partial<Record<ProviderId, string>>>;
  /** Deterministic capability-cache clock for tests. */
  readonly now?: () => number;
  /** Process seam for deterministic Pi adapter tests. */
  readonly pi?: PiProviderOptions;
}

export class ProviderRegistry {
  readonly #providers: ReadonlyMap<ProviderId, SmartProvider>;

  constructor(options: ProviderRegistryOptions = {}) {
    const transport = options.transport ?? new PinnedJsonTransport();
    const providers = new Map<ProviderId, SmartProvider>();
    for (const preset of OPENAI_COMPATIBLE_PRESETS) {
      providers.set(
        preset.id,
        createOpenAICompatibleProvider(preset, transport, {
          ...(options.endpointOverrides?.[preset.id] === undefined
            ? {}
            : { endpointOverride: options.endpointOverrides[preset.id] }),
        }),
      );
    }
    providers.set(
      'ollama',
      new OllamaProvider(transport, {
        ...(options.endpointOverrides?.ollama === undefined
          ? {}
          : { endpointOverride: options.endpointOverrides.ollama }),
        ...(options.now === undefined ? {} : { now: options.now }),
      }),
    );
    providers.set('pi', new PiProvider(options.pi));
    providers.set(
      'anthropic',
      new AnthropicProvider(transport, options.endpointOverrides?.anthropic),
    );
    providers.set('gemini', new GeminiProvider(transport, options.endpointOverrides?.gemini));
    providers.set('azure', new AzureOpenAIProvider(transport, options.endpointOverrides?.azure));
    providers.set('bedrock', new BedrockProvider(transport, options.endpointOverrides?.bedrock));
    providers.set('cohere', new CohereProvider(transport, options.endpointOverrides?.cohere));
    assertExactRegistry(providers);
    this.#providers = providers;
  }

  get(id: ProviderId): SmartProvider {
    const provider = this.#providers.get(id);
    if (provider === undefined) throw new Error('Provider registry is incomplete');
    return provider;
  }

  ids(): readonly ProviderId[] {
    return PROVIDER_REGISTRY_IDS;
  }

  catalog(): readonly ProviderCatalogEntry[] {
    return PROVIDER_CATALOG;
  }
}

const ollamaFields = Object.freeze([
  Object.freeze({
    key: 'baseUrl' as const,
    label: 'Endpoint URL',
    kind: 'url' as const,
    required: true,
    secret: false,
    placeholder: 'http://127.0.0.1:11434',
  }),
  Object.freeze({
    key: 'credential' as const,
    label: 'API key',
    kind: 'secret' as const,
    required: false,
    secret: true,
  }),
  Object.freeze({
    key: 'modelId' as const,
    label: 'Model',
    kind: 'model' as const,
    required: true,
    secret: false,
  }),
  Object.freeze({
    key: 'contextWindow' as const,
    label: 'Context window',
    kind: 'number' as const,
    required: false,
    secret: false,
    min: 1,
    description: 'Automatically uses the model limit, capped at 16384 when left empty.',
  }),
  Object.freeze({
    key: 'keepAlive' as const,
    label: 'Keep alive',
    kind: 'select' as const,
    required: true,
    secret: false,
    defaultValue: 300,
    options: Object.freeze([
      Object.freeze({ value: 0, label: 'Unload immediately' }),
      Object.freeze({ value: 300, label: '5 minutes' }),
      Object.freeze({ value: 1_800, label: '30 minutes' }),
      Object.freeze({ value: -1, label: 'Keep loaded' }),
    ]),
  }),
]);

function nativeFields(credentialLabel: string) {
  return Object.freeze([
    Object.freeze({
      key: 'credential' as const,
      label: credentialLabel,
      kind: 'secret' as const,
      required: true,
      secret: true,
    }),
    Object.freeze({
      key: 'modelId' as const,
      label: 'Model',
      kind: 'model' as const,
      required: true,
      secret: false,
    }),
  ]);
}

const piFields = Object.freeze([
  Object.freeze({
    key: 'modelId' as const,
    label: 'Pi model',
    kind: 'model' as const,
    required: true,
    secret: false,
    description: 'Choose an exact provider/model reported by the installed Pi CLI.',
  }),
  Object.freeze({
    key: 'thinking' as const,
    label: 'Thinking level',
    kind: 'select' as const,
    required: true,
    secret: false,
    defaultValue: 'off',
    options: Object.freeze(
      [
        { value: 'off', label: 'Off' },
        { value: 'minimal', label: 'Minimal' },
        { value: 'low', label: 'Low' },
        { value: 'medium', label: 'Medium' },
        { value: 'high', label: 'High' },
        { value: 'xhigh', label: 'Extra high' },
        { value: 'max', label: 'Max' },
      ].map((option) => Object.freeze(option)),
    ),
  }),
]);

const azureFields = Object.freeze([
  Object.freeze({
    key: 'baseUrl' as const,
    label: 'Azure resource endpoint',
    kind: 'url' as const,
    required: true,
    secret: false,
    placeholder: 'https://my-resource.openai.azure.com',
  }),
  Object.freeze({
    key: 'credential' as const,
    label: 'Azure OpenAI API key',
    kind: 'secret' as const,
    required: true,
    secret: true,
  }),
  Object.freeze({
    key: 'modelId' as const,
    label: 'Deployment name',
    kind: 'model' as const,
    required: true,
    secret: false,
  }),
  Object.freeze({
    key: 'contextWindow' as const,
    label: 'Context window',
    kind: 'number' as const,
    required: false,
    secret: false,
    min: 1,
    max: 2_000_000,
  }),
  Object.freeze({
    key: 'modelType' as const,
    label: 'Model type',
    kind: 'select' as const,
    required: true,
    secret: false,
    defaultValue: 'default',
    options: Object.freeze([
      Object.freeze({ value: 'default', label: 'Default' }),
      Object.freeze({ value: 'reasoning', label: 'Reasoning' }),
    ]),
  }),
]);

const bedrockFields = Object.freeze([
  Object.freeze({
    key: 'credential' as const,
    label: 'AWS credentials',
    kind: 'secret' as const,
    required: true,
    secret: true,
  }),
  Object.freeze({
    key: 'region' as const,
    label: 'AWS region',
    kind: 'select' as const,
    required: true,
    secret: false,
    defaultValue: 'us-west-2',
    options: Object.freeze(
      [
        'us-east-1',
        'us-east-2',
        'us-west-1',
        'us-west-2',
        'ca-central-1',
        'eu-north-1',
        'eu-west-1',
        'eu-west-2',
        'eu-west-3',
        'eu-central-1',
        'eu-south-1',
        'af-south-1',
        'ap-northeast-1',
        'ap-northeast-2',
        'ap-northeast-3',
        'ap-southeast-1',
        'ap-southeast-2',
        'ap-southeast-3',
        'ap-east-1',
        'ap-south-1',
        'sa-east-1',
        'me-south-1',
        'me-central-1',
      ].map((region) => Object.freeze({ value: region, label: region })),
    ),
  }),
  Object.freeze({
    key: 'modelId' as const,
    label: 'Model',
    kind: 'model' as const,
    required: true,
    secret: false,
  }),
  Object.freeze({
    key: 'contextWindow' as const,
    label: 'Context window',
    kind: 'number' as const,
    required: false,
    secret: false,
    min: 1,
    max: 2_000_000,
  }),
]);

const PROVIDER_REGISTRY_IDS: readonly ProviderId[] = Object.freeze([...PROVIDER_IDS]);

export const PROVIDER_CATALOG: readonly ProviderCatalogEntry[] = deepFreeze([
  ...OPENAI_COMPATIBLE_PRESETS.map((preset) =>
    ProviderCatalogEntrySchema.parse({
      id: preset.id,
      displayName: preset.displayName,
      description: preset.description,
      logo: preset.logo,
      destinationHint: preset.destinationHint,
      defaultModel: preset.defaultModel,
      modelDiscovery: 'remote',
      fields: preset.fields,
    }),
  ),
  ProviderCatalogEntrySchema.parse({
    id: 'ollama',
    displayName: 'Ollama',
    description: 'Run LLMs locally on your own machine.',
    logo: 'ollama.png',
    destinationHint: 'local',
    defaultModel: null,
    modelDiscovery: 'remote',
    fields: ollamaFields,
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'pi',
    displayName: 'Pi',
    description: 'Use your installed Pi CLI in print mode with tools and sessions disabled.',
    logo: 'pi.png',
    destinationHint: 'cloud',
    defaultModel: null,
    modelDiscovery: 'remote',
    fields: piFields,
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'anthropic',
    displayName: 'Anthropic',
    description: 'Use Claude through the native Anthropic Messages API.',
    logo: 'anthropic.png',
    destinationHint: 'cloud',
    defaultModel: 'claude-sonnet-4-6',
    modelDiscovery: 'remote',
    fields: nativeFields('Anthropic API key'),
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'gemini',
    displayName: 'Google Gemini',
    description: 'Use Gemini through the native Google Generative Language API.',
    logo: 'gemini.png',
    destinationHint: 'cloud',
    defaultModel: 'gemini-2.0-flash-lite',
    modelDiscovery: 'remote',
    fields: nativeFields('Google AI API key'),
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'azure',
    displayName: 'Azure OpenAI',
    description: 'Use an Azure OpenAI deployment through its native data-plane API.',
    logo: 'azure.png',
    destinationHint: 'cloud',
    defaultModel: null,
    modelDiscovery: 'configured',
    fields: azureFields,
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'bedrock',
    displayName: 'AWS Bedrock',
    description: 'Use foundation models through the native Bedrock Converse API.',
    logo: 'bedrock.png',
    destinationHint: 'cloud',
    defaultModel: null,
    modelDiscovery: 'remote',
    fields: bedrockFields,
  }),
  ProviderCatalogEntrySchema.parse({
    id: 'cohere',
    displayName: 'Cohere',
    description: 'Use Command models through the native Cohere v2 Chat API.',
    logo: 'cohere.png',
    destinationHint: 'cloud',
    defaultModel: 'command-a-03-2025',
    modelDiscovery: 'remote',
    fields: nativeFields('Cohere API key'),
  }),
]);

function deepFreeze<Value>(value: Value): Value {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const nested of Object.values(value)) deepFreeze(nested);
  }
  return value;
}

function assertExactRegistry(providers: ReadonlyMap<ProviderId, SmartProvider>): void {
  if (providers.size !== PROVIDER_IDS.length) throw new Error('Provider registry size mismatch');
  for (const id of PROVIDER_IDS) {
    if (providers.get(id)?.id !== id) throw new Error('Provider registry ID mismatch');
  }
}
