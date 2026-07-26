import { afterEach, describe, expect, it, vi } from 'vitest';
import { PinnedJsonTransport } from '../../app/src/main/providers/json-transport';
import { OllamaProvider } from '../../app/src/main/providers/ollama';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import {
  sendJson,
  startMockProviderServer,
  type MockProviderServer,
} from '../helpers/mock-provider-server';

const servers: MockProviderServer[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
});

describe('native Ollama provider', () => {
  it('uses tags, show, and chat with context, keep-alive, and dynamic vision metadata', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') {
        sendJson(response, {
          models: [{ name: 'llama3' }, { name: 'llava' }, { name: 'nomic-embed' }],
        });
        return;
      }
      if (request.url === '/api/show') {
        const name = readBody(request.body).model;
        if (name !== 'llama3' && name !== 'llava' && name !== 'nomic-embed') {
          sendJson(response, { error: 'missing' }, 404);
          return;
        }
        sendJson(response, {
          capabilities:
            name === 'llava'
              ? ['completion', 'vision']
              : name === 'nomic-embed'
                ? ['embedding']
                : ['completion'],
          model_info: { 'llama.context_length': name === 'llava' ? 32_768 : 8_192 },
        });
        return;
      }
      if (request.url === '/api/chat') {
        sendJson(response, {
          done: true,
          done_reason: 'stop',
          message: { role: 'assistant', content: 'clean transcript' },
        });
        return;
      }
      sendJson(response, {}, 404);
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });
    const invocation = {
      config: {
        providerId: 'ollama' as const,
        baseUrl: server.origin,
        modelId: 'llava',
        contextWindow: 12_345,
        keepAlive: 3_600,
        maxOutputTokens: 777,
      },
      credential: null,
    };

    await expect(provider.listModels(invocation, AbortSignal.timeout(2_000))).resolves.toEqual([
      { id: 'llama3', name: 'llama3', contextWindow: 8_192, vision: 'unsupported' },
      { id: 'llava', name: 'llava', contextWindow: 32_768, vision: 'supported' },
    ]);
    expect(provider.capabilities(invocation.config, 'llava')).toBe('supported');
    await expect(
      provider.cleanTranscript(
        invocation,
        {
          input: 'raw words',
          maxOutputTokens: 400,
          temperature: 0.2,
          image: { mimeType: 'image/jpeg', base64: '/9j/2Q==' },
        },
        AbortSignal.timeout(2_000),
      ),
    ).resolves.toBe('clean transcript');
    const chat = [...server.requests].reverse().find((request) => request.url === '/api/chat');
    expect(chat?.body).toMatchObject({
      model: 'llava',
      stream: false,
      keep_alive: 3_600,
      options: { num_ctx: 12_345, temperature: 0.2, num_predict: 400 },
      messages: [{ role: 'user', content: 'raw words', images: ['/9j/2Q=='] }],
    });
    await provider.cleanTranscript(invocation, { input: 'raw words' }, AbortSignal.timeout(2_000));
    const defaultedChat = [...server.requests]
      .reverse()
      .find((request) => request.url === '/api/chat');
    expect(defaultedChat?.body).toMatchObject({ options: { num_predict: 777 } });
    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).resolves.toMatchObject({
      ok: true,
      destination: 'local',
    });
  });

  it('fetches model details with bounded concurrency while preserving discovery order', async () => {
    const names = Array.from({ length: 12 }, (_, index) => `model-${String(index)}`);
    let active = 0;
    let maximumActive = 0;
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') {
        sendJson(response, { models: names.map((name) => ({ name })) });
        return;
      }
      if (request.url === '/api/show') {
        active += 1;
        maximumActive = Math.max(maximumActive, active);
        setTimeout(() => {
          active -= 1;
          sendJson(response, { capabilities: ['completion'], model_info: {} });
        }, 20);
        return;
      }
      sendJson(response, {}, 404);
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });

    const models = await provider.listModels(
      { config: { providerId: 'ollama', baseUrl: server.origin }, credential: null },
      AbortSignal.timeout(2_000),
    );

    expect(maximumActive).toBeGreaterThan(1);
    expect(maximumActive).toBeLessThanOrEqual(4);
    expect(models.map(({ id }) => id)).toEqual(names);
  });

  it('bypasses cached model details during explicit model refresh', async () => {
    let showCalls = 0;
    let contextWindow = 8_192;
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') {
        sendJson(response, { models: [{ name: 'refreshable-model' }] });
      } else if (request.url === '/api/show') {
        showCalls += 1;
        sendJson(response, {
          capabilities: ['completion'],
          model_info: { 'model.context_length': contextWindow },
        });
      } else {
        sendJson(response, {}, 404);
      }
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });
    const invocation = {
      config: { providerId: 'ollama' as const, baseUrl: server.origin },
      credential: null,
    };

    await expect(provider.listModels(invocation, AbortSignal.timeout(2_000))).resolves.toEqual([
      {
        id: 'refreshable-model',
        name: 'refreshable-model',
        contextWindow: 8_192,
        vision: 'unsupported',
      },
    ]);
    contextWindow = 32_768;
    await expect(provider.listModels(invocation, AbortSignal.timeout(2_000))).resolves.toEqual([
      expect.objectContaining({ contextWindow: 8_192 }),
    ]);
    await expect(
      provider.listModels({ ...invocation, refreshModels: true }, AbortSignal.timeout(2_000)),
    ).resolves.toEqual([expect.objectContaining({ contextWindow: 32_768 })]);
    expect(showCalls).toBe(2);
  });

  it('aborts and drains peer detail requests before returning the first failure', async () => {
    const names = Array.from({ length: 8 }, (_, index) => `failing-${String(index)}`);
    const requested: string[] = [];
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') {
        sendJson(response, { models: names.map((name) => ({ name })) });
        return;
      }
      if (request.url === '/api/show') {
        const name = String(readBody(request.body).model);
        requested.push(name);
        if (name === names[0]) {
          sendJson(response, { capabilities: 'malformed' });
        } else {
          setTimeout(() => {
            if (!response.destroyed) {
              sendJson(response, { capabilities: ['completion'], model_info: {} });
            }
          }, 100);
        }
        return;
      }
      sendJson(response, {}, 404);
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });

    await expect(
      provider.listModels(
        { config: { providerId: 'ollama', baseUrl: server.origin }, credential: null },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(requested.length).toBeLessThanOrEqual(4);
    expect(requested.every((name) => names.slice(0, 4).includes(name))).toBe(true);
  });

  it('probes cold model metadata and rejects a vision-named model declared non-vision', async () => {
    let showCalls = 0;
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/show') {
        showCalls += 1;
        sendJson(response, { capabilities: ['completion'], model_info: {} });
      } else if (request.url === '/api/chat') {
        sendJson(response, {
          done: true,
          message: { role: 'assistant', content: 'must not send' },
        });
      } else {
        sendJson(response, {}, 404);
      }
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });
    const invocation = {
      config: { providerId: 'ollama' as const, baseUrl: server.origin, modelId: 'llava' },
      credential: null,
    };

    expect(provider.capabilities(invocation.config, 'llava')).toBe('unknown');
    await expect(
      provider.cleanTranscript(
        invocation,
        {
          input: 'raw',
          image: { mimeType: 'image/jpeg', base64: '/9j/2Q==' },
        },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    expect(showCalls).toBe(1);
    expect(server.requests.some((request) => request.url === '/api/chat')).toBe(false);
    expect(provider.capabilities(invocation.config, 'llava')).toBe('unsupported');
  });

  it('preflights cold novel vision models and re-probes TTL and credential boundaries fail closed', async () => {
    let now = 1_000;
    let vision = true;
    let showCalls = 0;
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/show') {
        showCalls += 1;
        sendJson(response, {
          capabilities: vision ? ['completion', 'vision'] : ['completion'],
          model_info: { 'novel.context_length': 8_192 },
        });
      } else {
        sendJson(response, {}, 404);
      }
    });
    servers.push(server);
    let credential = 'credential-a';
    const service = new ProviderService(
      new ProviderRegistry({
        transport: new PinnedJsonTransport(),
        endpointOverrides: { ollama: server.origin },
        now: () => now,
      }),
      { getCredential: () => credential },
    );
    const config = {
      providerId: 'ollama' as const,
      baseUrl: server.origin,
      modelId: 'novel-vlm-without-name-hints',
    };

    await expect(
      service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('supported');
    expect(showCalls).toBe(1);

    // A different credential cannot reuse the first credential's affirmative capability cache.
    vision = false;
    credential = 'credential-b';
    await expect(
      service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('unsupported');
    expect(showCalls).toBe(2);

    // The original credential is supported only until its bounded cache entry expires.
    now += 5 * 60_000 + 1;
    credential = 'credential-a';
    await expect(
      service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('unsupported');
    expect(showCalls).toBe(3);
    service.dispose();
  });

  it('rejects incomplete or malformed non-stream chat envelopes', async () => {
    let chatBody: unknown = null;
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') {
        sendJson(response, { models: [{ name: 'chat-model' }] });
      } else if (request.url === '/api/show') {
        sendJson(response, {
          capabilities: ['completion'],
          model_info: { 'model.context_length': 8_192 },
        });
      } else if (request.url === '/api/chat') {
        sendJson(response, chatBody);
      } else {
        sendJson(response, {}, 404);
      }
    });
    servers.push(server);
    const provider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: server.origin,
    });
    const invocation = {
      config: {
        providerId: 'ollama' as const,
        baseUrl: server.origin,
        modelId: 'chat-model',
      },
      credential: null,
    };
    const invalidEnvelopes: readonly [string, unknown][] = [
      ['missing done', { message: { role: 'assistant', content: 'text' } }],
      ['done false', { done: false, message: { role: 'assistant', content: 'text' } }],
      [
        'explicit truncation',
        {
          done: true,
          done_reason: 'length',
          message: { role: 'assistant', content: 'partial text' },
        },
      ],
      ['wrong role', { done: true, message: { role: 'user', content: 'text' } }],
      ['missing message', { done: true }],
      ['malformed message', { done: true, message: 'assistant' }],
      ['missing content', { done: true, message: { role: 'assistant' } }],
      ['malformed content', { done: true, message: { role: 'assistant', content: 42 } }],
      ['empty content', { done: true, message: { role: 'assistant', content: '   ' } }],
      [
        'oversized content',
        { done: true, message: { role: 'assistant', content: 'x'.repeat(200_001) } },
      ],
    ];

    for (const [name, body] of invalidEnvelopes) {
      chatBody = body;
      await expect(
        provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
        name,
      ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    }

    chatBody = { message: { role: 'assistant', content: 'missing done' } };
    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
    });
  });

  it('distinguishes no models, selected model absent, and unavailable', async () => {
    const empty = await startMockProviderServer((_request, response) => {
      sendJson(response, { models: [] });
    });
    servers.push(empty);
    const emptyProvider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: empty.origin,
    });
    await expect(
      emptyProvider.listModels(
        { config: { providerId: 'ollama', baseUrl: empty.origin }, credential: null },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'NO_MODELS' });

    const installed = await startMockProviderServer((request, response) => {
      if (request.url === '/api/tags') sendJson(response, { models: [{ name: 'installed' }] });
      else sendJson(response, { capabilities: ['completion'], model_info: {} });
    });
    servers.push(installed);
    const installedProvider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: installed.origin,
    });
    await expect(
      installedProvider.validate(
        {
          config: { providerId: 'ollama', baseUrl: installed.origin, modelId: 'absent' },
          credential: null,
        },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'MODEL_NOT_FOUND' });

    const stopped = await startMockProviderServer((_request, response) => sendJson(response, {}));
    const stoppedOrigin = stopped.origin;
    await stopped.close();
    const unavailableProvider = new OllamaProvider(new PinnedJsonTransport(), {
      endpointOverride: stoppedOrigin,
    });
    await expect(
      unavailableProvider.listModels(
        { config: { providerId: 'ollama', baseUrl: stoppedOrigin }, credential: null },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE' });
  });
});

describe('provider service seam', () => {
  it('bounds the complete operation while a credential resolver is stalled', async () => {
    vi.useFakeTimers();
    try {
      const service = new ProviderService(new ProviderRegistry(), {
        getCredential: () => new Promise(() => undefined),
      });
      const pending = service.listModels(
        {
          providerId: 'generic-openai',
          baseUrl: 'http://127.0.0.1:8000/v1',
          modelId: 'model',
        },
        new AbortController().signal,
      );
      const rejection = expect(pending).rejects.toMatchObject({ code: 'TIMEOUT' });
      await vi.advanceTimersByTimeAsync(30_000);
      await rejection;
    } finally {
      vi.useRealTimers();
    }
  });

  it('resolves credentials only in main and supports cancellation-safe registry operations', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.method === 'GET') sendJson(response, { data: [{ id: 'model' }] });
      else sendJson(response, { choices: [{ message: { content: 'clean' } }] });
    });
    servers.push(server);
    const registry = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { 'generic-openai': server.origin },
    });
    const service = new ProviderService(registry, {
      getCredential: (id) => (id === 'generic-openai' ? 'vault-secret' : null),
    });
    const config = {
      providerId: 'generic-openai' as const,
      baseUrl: server.origin,
      modelId: 'model',
    };
    await expect(service.listModels(config, AbortSignal.timeout(2_000))).resolves.toHaveLength(1);
    await expect(
      service.cleanTranscript(
        config,
        { input: 'raw', temperature: 0.2, maxOutputTokens: 100 },
        AbortSignal.timeout(2_000),
      ),
    ).resolves.toBe('clean');
    expect(
      server.requests.every((request) => request.headers.authorization === 'Bearer vault-secret'),
    ).toBe(true);
    expect(JSON.stringify(service.catalog())).not.toContain('vault-secret');

    const cancelled = new AbortController();
    cancelled.abort();
    await expect(service.listModels(config, cancelled.signal)).rejects.toMatchObject({
      code: 'CANCELLED',
    });

    const waitingForVault = new ProviderService(registry, {
      getCredential: () => new Promise(() => undefined),
    });
    const duringCredentialRead = new AbortController();
    const pending = waitingForVault.listModels(config, duringCredentialRead.signal);
    duringCredentialRead.abort();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED' });
  });
});

function readBody(input: unknown): Readonly<Record<string, unknown>> {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) {
    throw new Error('invalid request body');
  }
  return input as Readonly<Record<string, unknown>>;
}
