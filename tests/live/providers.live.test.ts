import { describe, expect, it } from 'vitest';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';
import type { ProviderConfig, ProviderId } from '../../app/src/shared/schemas/providers';

const runOllama = process.env.TALKING_QUILL_LIVE_OLLAMA === '1';
const runPiList = process.env.TALKING_QUILL_LIVE_PI_LIST === '1';
const openAiKey = process.env.TALKING_QUILL_LIVE_OPENAI_KEY;
const bedrockAccessKeyId = process.env.TALKING_QUILL_LIVE_BEDROCK_ACCESS_KEY_ID;
const bedrockSecretAccessKey = process.env.TALKING_QUILL_LIVE_BEDROCK_SECRET_ACCESS_KEY;
const bedrockCredential =
  bedrockAccessKeyId === undefined || bedrockSecretAccessKey === undefined
    ? undefined
    : serializeAwsCredentials({
        accessKeyId: bedrockAccessKeyId,
        secretAccessKey: bedrockSecretAccessKey,
        ...(process.env.TALKING_QUILL_LIVE_BEDROCK_SESSION_TOKEN === undefined
          ? {}
          : { sessionToken: process.env.TALKING_QUILL_LIVE_BEDROCK_SESSION_TOKEN }),
      });
const nativeCases: readonly {
  readonly id: ProviderId;
  readonly credential: string | undefined;
  readonly config: ProviderConfig | null;
}[] = [
  {
    id: 'anthropic',
    credential: process.env.TALKING_QUILL_LIVE_ANTHROPIC_KEY,
    config:
      process.env.TALKING_QUILL_LIVE_ANTHROPIC_MODEL === undefined
        ? null
        : { providerId: 'anthropic', modelId: process.env.TALKING_QUILL_LIVE_ANTHROPIC_MODEL },
  },
  {
    id: 'gemini',
    credential: process.env.TALKING_QUILL_LIVE_GEMINI_KEY,
    config:
      process.env.TALKING_QUILL_LIVE_GEMINI_MODEL === undefined
        ? null
        : { providerId: 'gemini', modelId: process.env.TALKING_QUILL_LIVE_GEMINI_MODEL },
  },
  {
    id: 'azure',
    credential: process.env.TALKING_QUILL_LIVE_AZURE_KEY,
    config:
      process.env.TALKING_QUILL_LIVE_AZURE_ENDPOINT === undefined ||
      process.env.TALKING_QUILL_LIVE_AZURE_DEPLOYMENT === undefined
        ? null
        : {
            providerId: 'azure',
            baseUrl: process.env.TALKING_QUILL_LIVE_AZURE_ENDPOINT,
            modelId: process.env.TALKING_QUILL_LIVE_AZURE_DEPLOYMENT,
            modelType:
              process.env.TALKING_QUILL_LIVE_AZURE_MODEL_TYPE === 'reasoning'
                ? 'reasoning'
                : 'default',
          },
  },
  {
    id: 'bedrock',
    credential: bedrockCredential,
    config:
      process.env.TALKING_QUILL_LIVE_BEDROCK_MODEL === undefined
        ? null
        : {
            providerId: 'bedrock',
            region: process.env.TALKING_QUILL_LIVE_BEDROCK_REGION ?? 'us-west-2',
            modelId: process.env.TALKING_QUILL_LIVE_BEDROCK_MODEL,
          },
  },
  {
    id: 'cohere',
    credential: process.env.TALKING_QUILL_LIVE_COHERE_KEY,
    config:
      process.env.TALKING_QUILL_LIVE_COHERE_MODEL === undefined
        ? null
        : { providerId: 'cohere', modelId: process.env.TALKING_QUILL_LIVE_COHERE_MODEL },
  },
];

const registry = new ProviderRegistry();
const RED_JPEG_BASE64 =
  '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCABAAEADASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAYI/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AnQCOaRAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAf//Z';

describe.skipIf(!runPiList)('live Pi model discovery (opt-in, non-billable)', () => {
  it('resolves the canonical installed package and parses its real model table', async () => {
    const provider = registry.get('pi');
    const models = await provider.listModels(
      { config: { providerId: 'pi' }, credential: null },
      AbortSignal.timeout(30_000),
    );
    expect(models.length).toBeGreaterThan(0);
    expect(models.every(({ id }) => id.includes('/'))).toBe(true);
  });
});

describe.skipIf(!runOllama)('live Ollama smoke (opt-in)', () => {
  it('requires and executes both text and vision models', async () => {
    const endpoint = process.env.TALKING_QUILL_LIVE_OLLAMA_URL ?? 'http://127.0.0.1:11434';
    const requestedModel = process.env.TALKING_QUILL_LIVE_OLLAMA_MODEL;
    const requestedVisionModel = process.env.TALKING_QUILL_LIVE_OLLAMA_VISION_MODEL;
    const service = new ProviderService(registry, { getCredential: () => null });
    const baseConfig = { providerId: 'ollama' as const, baseUrl: endpoint };
    const models = await service.listModels(baseConfig, new AbortController().signal);
    const modelId = requestedModel ?? models.at(0)?.id;
    if (modelId === undefined) {
      throw new Error('Install an Ollama chat model or set TALKING_QUILL_LIVE_OLLAMA_MODEL');
    }
    const result = await service.testConnection(
      { ...baseConfig, modelId },
      new AbortController().signal,
    );
    expect(result.ok).toBe(true);
    expect(result.destination).toBe('local');
    expect(service.capabilities({ ...baseConfig, modelId }, modelId)).not.toBe('unknown');
    const cleaned = await service.cleanTranscript(
      { ...baseConfig, modelId },
      {
        input: 'this is a short transcript that needs punctuation',
        temperature: 0.2,
        maxOutputTokens: 64,
      },
      new AbortController().signal,
    );
    expect(cleaned.trim().length).toBeGreaterThan(0);

    const visionModelId =
      requestedVisionModel ?? models.find((model) => model.vision === 'supported')?.id;
    if (visionModelId === undefined) {
      throw new Error(
        'Install a vision model or set TALKING_QUILL_LIVE_OLLAMA_VISION_MODEL; enabled live runs require vision.',
      );
    }
    if (!models.some((model) => model.id === visionModelId)) {
      throw new Error(`The requested Ollama vision model is not installed: ${visionModelId}`);
    }
    expect(service.capabilities({ ...baseConfig, modelId: visionModelId }, visionModelId)).toBe(
      'supported',
    );
    const vision = await service.cleanTranscript(
      { ...baseConfig, modelId: visionModelId },
      {
        input: 'Return only the dominant color in this image.',
        image: { mimeType: 'image/jpeg', base64: RED_JPEG_BASE64 },
        temperature: 0.2,
        maxOutputTokens: 32,
      },
      new AbortController().signal,
    );
    expect(vision.toLowerCase()).toContain('red');
  }, 120_000);
});

describe.each(nativeCases)(
  'live native provider $id smoke (opt-in, billable)',
  ({ id, credential, config }) => {
    it.skipIf(credential === undefined || config === null)(
      'lists, validates, and completes using native APIs',
      async () => {
        if (config === null) throw new Error('A model configuration is required');
        const service = new ProviderService(registry, {
          getCredential: (providerId) => (providerId === id ? (credential ?? null) : null),
        });
        const models = await service.listModels(config, new AbortController().signal);
        expect(models.some((model) => model.id === config.modelId)).toBe(true);
        await expect(
          service.testConnection(config, new AbortController().signal),
        ).resolves.toMatchObject({ ok: true });
        await expect(
          service.cleanTranscript(
            config,
            { input: 'add punctuation to this short sentence', maxOutputTokens: 64 },
            new AbortController().signal,
          ),
        ).resolves.toEqual(expect.any(String));
      },
      120_000,
    );
  },
);

describe.skipIf(openAiKey === undefined)('live OpenAI smoke (opt-in, billable)', () => {
  it('lists models and validates API-key authentication', async () => {
    const service = new ProviderService(registry, {
      getCredential: (providerId) => (providerId === 'openai' ? (openAiKey ?? null) : null),
    });
    const result = await service.testConnection(
      {
        providerId: 'openai',
        modelId: process.env.TALKING_QUILL_LIVE_OPENAI_MODEL ?? 'gpt-4.1-nano',
      },
      new AbortController().signal,
    );
    expect(result.ok).toBe(true);
    expect(result.destination).toBe('cloud');
  }, 120_000);
});
