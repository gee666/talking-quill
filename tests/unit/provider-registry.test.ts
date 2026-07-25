import { describe, expect, it } from 'vitest';
import {
  OPENAI_COMPATIBLE_PROVIDER_IDS,
  PROVIDER_IDS,
  NATIVE_CLOUD_PROVIDER_IDS,
  ProviderCatalogEntrySchema,
  ProviderConfigSchema,
} from '../../app/src/shared/schemas/providers';
import type { JsonTransport } from '../../app/src/main/providers/json-transport';
import { OPENAI_COMPATIBLE_PRESETS } from '../../app/src/main/providers/presets';
import { ProviderRegistry } from '../../app/src/main/providers/registry';

describe('provider registry and presets', () => {
  it('contains exactly 31 immutable presets, Ollama, and five native cloud adapters', () => {
    expect(OPENAI_COMPATIBLE_PROVIDER_IDS).toHaveLength(31);
    expect(NATIVE_CLOUD_PROVIDER_IDS).toEqual([
      'anthropic',
      'gemini',
      'azure',
      'bedrock',
      'cohere',
    ]);
    expect(PROVIDER_IDS).toHaveLength(38);
    expect(new Set(PROVIDER_IDS).size).toBe(38);
    expect(OPENAI_COMPATIBLE_PRESETS.map(({ id }) => id)).toEqual(OPENAI_COMPATIBLE_PROVIDER_IDS);
    expect(isDeepFrozen(OPENAI_COMPATIBLE_PRESETS)).toBe(true);

    const registry = new ProviderRegistry({ transport: noNetworkTransport() });
    expect(registry.ids()).toEqual(PROVIDER_IDS);
    expect(registry.catalog()).toHaveLength(38);
    expect(isDeepFrozen(registry.catalog())).toBe(true);
    expect(Object.isFrozen(registry.ids())).toBe(true);
    for (const entry of registry.catalog())
      expect(ProviderCatalogEntrySchema.parse(entry)).toEqual(entry);
  });

  it('keeps the required runtime truth overrides and data-only metadata', () => {
    const byId = Object.fromEntries(OPENAI_COMPATIBLE_PRESETS.map((preset) => [preset.id, preset]));
    expect(byId.openai?.protocol).toBe('responses');
    expect(byId.openai?.completionPath).toBe('responses');
    expect(byId.perplexity?.defaultModel).toBe('sonar');
    const perplexityModels = byId.perplexity?.modelList;
    expect(perplexityModels?.kind).toBe('static');
    if (perplexityModels?.kind !== 'static') throw new Error('Perplexity models are missing');
    expect(perplexityModels.models.find(({ id }) => id === 'sonar-pro')).toEqual({
      id: 'sonar-pro',
      contextWindow: 200_000,
    });
    expect(byId['docker-model-runner']?.endpoint.normalization).toBe('origin-engines-v1');
    expect(byId['docker-model-runner']?.defaultContextWindow).toBe(8_192);
    expect(byId['docker-model-runner']?.modelList).toMatchObject({ path: '/models' });
    expect(JSON.stringify(byId['docker-model-runner'])).not.toMatch(/docker\.io|hub/i);
    expect(byId.lemonade?.defaultContextWindow).toBe(8_192);
    expect(byId.lmstudio?.defaultContextWindow).toBe(16_384);
    expect(byId.localai?.auth).toBe('optional-bearer');
    expect(OPENAI_COMPATIBLE_PRESETS.map((preset) => preset.logo)).toEqual([
      'openai.png',
      'generic-openai.png',
      'lmstudio.png',
      'localai.png',
      'koboldcpp.png',
      'text-generation-webui.png',
      'docker-model-runner.png',
      'lemonade.png',
      'foundry-local.png',
      'omlx.png',
      'groq.png',
      'openrouter.jpeg',
      'togetherai.png',
      'fireworksai.jpeg',
      'deepseek.png',
      'perplexity.png',
      'mistral.jpeg',
      'novita.png',
      'cometapi.png',
      'ppio.png',
      'apipie.png',
      'sambanova.png',
      'cerebras.png',
      'giteeai.png',
      'minimax.png',
      'moonshotai.png',
      'zai.png',
      'xai.png',
      'nvidia-nim.png',
      'privatemode.png',
      'litellm.png',
    ]);
    expect(OPENAI_COMPATIBLE_PRESETS.every((preset) => preset.fields.every(isDataOnly))).toBe(true);
    expect(JSON.stringify(OPENAI_COMPATIBLE_PRESETS)).not.toContain('AnythingLLM');
  });

  it('uses strict shared provider configuration schemas', () => {
    expect(
      ProviderConfigSchema.parse({ providerId: 'ollama', baseUrl: 'http://127.0.0.1:11434' }),
    ).toEqual({ providerId: 'ollama', baseUrl: 'http://127.0.0.1:11434' });
    expect(() => ProviderConfigSchema.parse({ providerId: 'unknown' })).toThrow();
    expect(() => ProviderConfigSchema.parse({ providerId: 'ollama', secret: 'leak' })).toThrow();
    expect(() => ProviderConfigSchema.parse({ providerId: 'ollama' })).toThrow();
    expect(() =>
      ProviderConfigSchema.parse({
        providerId: 'ollama',
        baseUrl: 'http://127.0.0.1:11434?key=secret',
      }),
    ).toThrow();
    expect(() =>
      ProviderConfigSchema.parse({ providerId: 'openai', baseUrl: 'https://override.invalid' }),
    ).toThrow();
    expect(() => ProviderConfigSchema.parse({ providerId: 'openai', keepAlive: 300 })).toThrow();
    expect(() => ProviderConfigSchema.parse({ providerId: 'openai', timeoutMs: 3_000 })).toThrow();
    expect(
      ProviderConfigSchema.parse({ providerId: 'openrouter', timeoutMs: 3_000 }),
    ).toMatchObject({ providerId: 'openrouter', timeoutMs: 3_000 });
  });

  it('registers every native cloud adapter as credentialed and runnable', () => {
    const registry = new ProviderRegistry({ transport: noNetworkTransport() });
    for (const id of NATIVE_CLOUD_PROVIDER_IDS) {
      const provider = registry.get(id);
      expect(provider.id).toBe(id);
      expect(provider.credentialPolicy).toBe('required');
      expect(registry.catalog().find((entry) => entry.id === id)?.fields.length).toBeGreaterThan(0);
    }
    expect(registry.catalog().every((entry) => !('implemented' in entry))).toBe(true);
  });
});

function noNetworkTransport(): JsonTransport {
  return {
    request: () => Promise.reject(new Error('unexpected network')),
    classify: () => Promise.reject(new Error('unexpected network')),
  };
}

function isDeepFrozen(value: unknown): boolean {
  if (value === null || typeof value !== 'object') return true;
  return Object.isFrozen(value) && Object.values(value).every(isDeepFrozen);
}

function isDataOnly(value: unknown): boolean {
  if (typeof value === 'function') return false;
  if (value === null || typeof value !== 'object') return true;
  return Object.values(value).every(isDataOnly);
}
