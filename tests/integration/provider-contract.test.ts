import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  CONFIGURABLE_PROVIDER_IDS,
  NATIVE_CLOUD_PROVIDER_IDS,
  PROVIDER_IDS,
  type OpenAICompatibleProviderId,
  type ProviderConfig,
} from '../../app/src/shared/schemas/providers';
import {
  PinnedJsonTransport,
  type JsonTransport,
  type JsonTransportRequest,
} from '../../app/src/main/providers/json-transport';
import { ProviderError } from '../../app/src/main/providers/errors';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';
import { OPENAI_COMPATIBLE_PRESETS } from '../../app/src/main/providers/presets';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import type { PiCliIdentity } from '../../app/src/main/providers/pi';
import {
  sendJson,
  startMockProviderServer,
  type MockProviderServer,
} from '../helpers/mock-provider-server';

const servers: MockProviderServer[] = [];
afterEach(async () => {
  vi.useRealTimers();
  await Promise.all(servers.splice(0).map((server) => server.close()));
});

describe('38-provider registry contract', () => {
  it('contains 31 compatible providers, native Ollama, and five runnable native cloud adapters', () => {
    const registry = new ProviderRegistry();
    expect(registry.ids()).toEqual(PROVIDER_IDS);
    expect(NATIVE_CLOUD_PROVIDER_IDS).toHaveLength(5);
    expect(registry.catalog().map(({ id }) => id)).toEqual(PROVIDER_IDS);
    expect(registry.catalog().every(({ fields }) => fields.length > 0)).toBe(true);
    for (const id of NATIVE_CLOUD_PROVIDER_IDS) {
      expect(registry.get(id)).toMatchObject({ id, credentialPolicy: 'required' });
    }
  });

  it('honors immediate cancellation before every one of the 38 provider IDs', async () => {
    const service = new ProviderService(new ProviderRegistry(), { getCredential: () => null });
    for (const providerId of PROVIDER_IDS) {
      const controller = new AbortController();
      controller.abort();
      const config = { providerId } as ProviderConfig;
      for (const operation of [
        service.listModels(config, controller.signal),
        service.testConnection(config, controller.signal),
        service.classifyDestination(config, controller.signal),
        service.cleanTranscript(config, { input: 'raw' }, controller.signal),
      ]) {
        await expect(operation, providerId).rejects.toMatchObject({ code: 'CANCELLED' });
      }
    }
  });

  it('propagates in-flight cancellation and enforces a real deadline for all 38 adapters', async () => {
    vi.useFakeTimers();
    for (const providerId of PROVIDER_IDS) {
      const config = contractConfig(providerId);
      const credential =
        providerId === 'bedrock'
          ? serializeAwsCredentials({
              accessKeyId: 'AKIDCONTRACT123456',
              secretAccessKey: 'contract-secret-access-key',
            })
          : 'contract-api-key';

      const cancellation = new AbortController();
      const cancelledProvider = new ProviderRegistry({
        transport: blockingTransport(),
        endpointOverrides: { [providerId]: 'http://127.0.0.1:43210' },
        ...(providerId === 'pi' ? { pi: piContractOptions(true) } : {}),
      }).get(providerId);
      const cancelled = cancelledProvider.cleanTranscript(
        { config, credential },
        {
          input: 'raw transcript',
          modelId: providerId === 'pi' ? 'contract/model' : 'contract-model',
        },
        cancellation.signal,
      );
      cancellation.abort();
      await expect(cancelled, `${providerId}: cancellation`).rejects.toMatchObject({
        code: 'CANCELLED',
      });

      const timedService = new ProviderService(
        new ProviderRegistry({
          transport: blockingTransport(),
          endpointOverrides: { [providerId]: 'http://127.0.0.1:43210' },
          ...(providerId === 'pi' ? { pi: piContractOptions(true) } : {}),
        }),
        { getCredential: () => credential },
        { operationTimeoutMs: 5 },
      );
      const timed = timedService.cleanTranscript(
        config,
        {
          input: 'raw transcript',
          modelId: providerId === 'pi' ? 'contract/model' : 'contract-model',
        },
        new AbortController().signal,
      );
      const timeoutExpectation = expect(timed, `${providerId}: timeout`).rejects.toMatchObject({
        code: 'TIMEOUT',
      });
      await vi.advanceTimersByTimeAsync(providerId === 'pi' ? 500 : 5);
      await timeoutExpectation;
    }
  });
});

describe('OpenAI-compatible provider contract', () => {
  it('executes request, auth, and model contracts for every one of the 31 presets', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url.endsWith('/health')) {
        sendJson(response, { all_models_loaded: [] });
        return;
      }
      if (request.url.endsWith('/load')) {
        sendJson(response, { status: 'success' });
        return;
      }
      if (request.method === 'GET') {
        sendJson(response, {
          data: [
            {
              id: 'gpt-test',
              name: 'gpt-test',
              context_length: 16_384,
              max_model_len: 16_384,
              type: 'chat',
              subtype: 'chat',
              tasks: ['generate'],
              tags: ['gpt-test'],
              labels: ['chat'],
              max_tokens: 16_384,
              limits: { max_context_length: 16_384 },
              config: { gguf: { 'test.context_length': 16_384 } },
            },
          ],
        });
        return;
      }
      if (request.url.endsWith('/responses')) {
        sendJson(response, {
          status: 'completed',
          output: [
            {
              type: 'message',
              role: 'assistant',
              content: [{ type: 'output_text', text: 'clean response' }],
            },
          ],
        });
      } else sendJson(response, { choices: [{ message: { content: 'clean response' } }] });
    });
    servers.push(server);
    const overrides = Object.fromEntries(
      OPENAI_COMPATIBLE_PRESETS.map((preset) => [preset.id, server.origin]),
    );
    const registry = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: overrides,
    });

    for (const preset of OPENAI_COMPATIBLE_PRESETS) {
      const start = server.requests.length;
      const credential = preset.auth === 'none' ? null : `key-${preset.id}`;
      const selectedModel =
        preset.id === 'textgenwebui'
          ? null
          : preset.modelList.kind === 'static'
            ? (preset.defaultModel ?? preset.modelList.models[0]?.id ?? 'gpt-test')
            : 'gpt-test';
      const config: ProviderConfig = {
        providerId: preset.id,
        ...(preset.endpoint.kind === 'configurable' ? { baseUrl: server.origin } : {}),
        ...(selectedModel === null ? {} : { modelId: selectedModel }),
      };
      const provider = registry.get(preset.id);
      const validationConfig: ProviderConfig = config;
      await expect(
        provider.validate({ config: validationConfig, credential }, AbortSignal.timeout(2_000)),
        preset.id,
      ).resolves.toMatchObject({ ok: true, destination: 'local' });
      const models = await provider.listModels({ config, credential }, AbortSignal.timeout(2_000));
      if (preset.modelList.kind === 'none') expect(models).toEqual([]);
      else expect(models.length).toBeGreaterThan(0);
      await expect(
        provider.cleanTranscript(
          { config, credential },
          {
            input: 'raw transcript',
            ...(preset.id === 'textgenwebui' ? {} : { modelId: 'gpt-test' }),
            temperature: 0.2,
            maxOutputTokens: 512,
          },
          AbortSignal.timeout(2_000),
        ),
      ).resolves.toBe('clean response');

      const captured = server.requests.slice(start);
      const completion = captured.find(
        (request) => request.method === 'POST' && !request.url.endsWith('/load'),
      );
      expect(completion, preset.id).toBeDefined();
      expect(completion?.headers['accept-encoding']).toBe('identity');
      expect(JSON.stringify(completion?.headers)).not.toContain('AnythingLLM');
      expect(completion?.headers['http-referer']).toBeUndefined();
      expect(completion?.headers['x-title']).toBeUndefined();
      if (preset.id === 'foundry') {
        expect(completion?.body).toHaveProperty('max_completion_tokens');
        expect(completion?.body).not.toHaveProperty('max_tokens');
      }
      if (preset.id === 'textgenwebui') expect(completion?.body).not.toHaveProperty('model');
      if (preset.auth === 'none') expect(completion?.headers.authorization).toBeUndefined();
      else expect(completion?.headers.authorization).toBe(`Bearer key-${preset.id}`);
      const modelRequest = captured.find((request) => request.method === 'GET');
      if (preset.modelList.kind === 'http') {
        if (preset.modelList.public || preset.auth === 'none') {
          expect(modelRequest?.headers.authorization).toBeUndefined();
        } else {
          expect(modelRequest?.headers.authorization).toBe(`Bearer key-${preset.id}`);
        }
      }
    }
  });

  it('keeps Text Generation WebUI model-less while accepting explicit legacy overrides', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, {
        choices: [{ finish_reason: 'stop', message: { content: 'clean response' } }],
      }),
    );
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { textgenwebui: server.origin },
    }).get('textgenwebui');
    const invocation = {
      config: { providerId: 'textgenwebui' as const, baseUrl: server.origin },
      credential: null,
    };

    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).resolves.toMatchObject({
      ok: true,
      destination: 'local',
      modelCount: 0,
    });
    expect(server.requests.at(-1)?.body).not.toHaveProperty('model');
    await expect(
      provider.cleanTranscript(
        invocation,
        { input: 'raw', modelId: 'manual-override' },
        AbortSignal.timeout(2_000),
      ),
    ).resolves.toBe('clean response');
    expect(server.requests.at(-1)?.body).toMatchObject({ model: 'manual-override' });
  });

  it('does not let preset defaults bypass required model selection', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, { data: [{ id: 'gpt-4.1-nano', owned_by: 'openai' }] }),
    );
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { openai: server.origin },
    }).get('openai');
    const invocation = {
      config: { providerId: 'openai' as const },
      credential: 'test-secret',
    };

    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).rejects.toMatchObject({
      code: 'INVALID_CONFIG',
    });
    await expect(
      provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    expect(server.requests.every(({ method }) => method === 'GET')).toBe(true);
  });

  it('uses the OpenAI Responses API shape and the declarative endpoint normalizers', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url.endsWith('/health')) sendJson(response, { all_models_loaded: [] });
      else if (request.url.endsWith('/load')) sendJson(response, { status: 'success' });
      else if (request.method === 'GET')
        sendJson(response, { data: [{ id: 'gpt-test', labels: ['chat'] }] });
      else if (request.url.endsWith('/responses')) {
        sendJson(response, {
          status: 'completed',
          output: [
            {
              type: 'message',
              role: 'assistant',
              content: [{ type: 'output_text', text: 'clean' }],
            },
          ],
        });
      } else sendJson(response, { choices: [{ message: { content: 'clean' } }] });
    });
    servers.push(server);
    const openai = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { openai: server.origin },
    }).get('openai');
    await openai.cleanTranscript(
      { config: { providerId: 'openai', modelId: 'gpt-4.1-nano' }, credential: 'secret' },
      { input: 'raw', temperature: 0.2, maxOutputTokens: 321 },
      AbortSignal.timeout(2_000),
    );
    expect(server.requests[0]?.url).toBe('/responses');
    expect(server.requests[0]?.body).toEqual({
      model: 'gpt-4.1-nano',
      input: 'raw',
      store: false,
      max_output_tokens: 321,
      temperature: 0.2,
    });
    await openai.cleanTranscript(
      { config: { providerId: 'openai', modelId: 'gpt-4.1-nano' }, credential: 'secret' },
      {
        input: 'raw with image',
        image: { mimeType: 'image/jpeg', base64: '/9j/2Q==' },
      },
      AbortSignal.timeout(2_000),
    );
    expect(server.requests.at(-1)?.body).toMatchObject({
      input: [
        {
          role: 'user',
          content: [
            { type: 'input_text', text: 'raw with image' },
            { type: 'input_image', image_url: 'data:image/jpeg;base64,/9j/2Q==' },
          ],
        },
      ],
    });
    await openai.cleanTranscript(
      { config: { providerId: 'openai', modelId: 'o3-mini' }, credential: 'secret' },
      { input: 'raw', temperature: 0.2 },
      AbortSignal.timeout(2_000),
    );
    expect(server.requests.at(-1)?.body).toMatchObject({
      model: 'o3-mini',
      temperature: 1,
    });

    const generic = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { 'generic-openai': server.origin },
    }).get('generic-openai');
    await generic.cleanTranscript(
      {
        config: {
          providerId: 'generic-openai',
          baseUrl: server.origin,
          modelId: 'gpt-test',
        },
        credential: null,
      },
      {
        input: 'raw',
        image: { mimeType: 'image/jpeg', base64: '/9j/2Q==' },
      },
      AbortSignal.timeout(2_000),
    );
    expect(server.requests.at(-1)?.body).toMatchObject({
      max_tokens: 1_024,
      temperature: 0.2,
      messages: [
        {
          content: [
            { type: 'text', text: 'raw' },
            {
              type: 'image_url',
              image_url: { url: 'data:image/jpeg;base64,/9j/2Q==' },
            },
          ],
        },
      ],
    });

    const normalized = await captureCompletionPaths(server.origin);
    expect(normalized['docker-model-runner']).toBe('/engines/v1/chat/completions');
    expect(normalized.lemonade).toBe('/api/v1/chat/completions');
    expect(normalized.lmstudio).toBe('/v1/chat/completions');
  });

  it('accepts only completed Responses API message output_text content', async () => {
    const fixtures = [
      { status: 'in_progress', output: [] },
      {
        status: 'completed',
        output: [{ type: 'message', content: [{ type: 'refusal', refusal: 'cannot comply' }] }],
      },
      { status: 'completed', output: [] },
      {
        status: 'completed',
        output: [
          {
            type: 'message',
            role: 'assistant',
            content: [{ type: 'output_text', text: 'cleaned transcript' }],
          },
        ],
      },
    ];
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, fixtures.shift()),
    );
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { openai: server.origin },
    }).get('openai');
    const invocation = {
      config: { providerId: 'openai' as const, modelId: 'gpt-4.1-nano' },
      credential: 'secret',
    };
    for (let index = 0; index < 3; index += 1) {
      await expect(
        provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
      ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    }
    await expect(
      provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
    ).resolves.toBe('cleaned transcript');
  });

  it('accepts Responses token exhaustion only for connection validation', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.method === 'GET') {
        sendJson(response, { data: [{ id: 'gpt-5-test', owned_by: 'openai' }] });
      } else {
        sendJson(response, {
          status: 'incomplete',
          incomplete_details: { reason: 'max_output_tokens' },
          output: [],
        });
      }
    });
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { openai: server.origin },
    }).get('openai');
    const invocation = {
      config: { providerId: 'openai' as const, modelId: 'gpt-5-test' },
      credential: 'test-secret',
    };

    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).resolves.toMatchObject({
      ok: true,
      destination: 'local',
    });
    await expect(
      provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });

  it('rejects explicitly truncated chat-completions output', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, {
        choices: [
          {
            finish_reason: 'length',
            message: { role: 'assistant', content: 'partial transcript' },
          },
        ],
      }),
    );
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { 'generic-openai': server.origin },
    }).get('generic-openai');

    await expect(
      provider.cleanTranscript(
        {
          config: {
            providerId: 'generic-openai',
            baseUrl: server.origin,
            modelId: 'gpt-test',
          },
          credential: null,
        },
        { input: 'raw' },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });

  it('normalizes provider-specific model-list shapes and filters declaratively', async () => {
    await expect(
      listModelsFromMock('lmstudio', {
        data: [
          { id: 'lm-chat', type: 'llm', max_context_length: 12_345 },
          { id: 'vector-model', type: 'embeddings', max_context_length: 2_048 },
        ],
      }),
    ).resolves.toEqual([
      { id: 'lm-chat', name: 'lm-chat', contextWindow: 12_345, vision: 'unknown' },
    ]);
    const legacyLmStudio = await startMockProviderServer((request, response) => {
      if (request.url === '/api/v0/models') {
        sendJson(response, { error: 'unsupported' }, 500);
        return;
      }
      sendJson(response, { data: [{ id: 'legacy-chat' }] });
    });
    servers.push(legacyLmStudio);
    await expect(
      new ProviderRegistry({
        endpointOverrides: { lmstudio: legacyLmStudio.origin },
      })
        .get('lmstudio')
        .listModels(
          {
            config: { providerId: 'lmstudio', baseUrl: legacyLmStudio.origin },
            credential: null,
          },
          AbortSignal.timeout(2_000),
        ),
    ).resolves.toEqual([
      {
        id: 'legacy-chat',
        name: 'legacy-chat',
        contextWindow: 16_384,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('docker-model-runner', [
        {
          tags: ['ai/chat:latest', 'ai/embed-model:latest', 'ai/all-mini-l6:latest'],
          config: { gguf: { 'llama.context_length': 8_192 } },
        },
      ]),
    ).resolves.toEqual([
      {
        id: 'ai/chat:latest',
        name: 'chat:latest',
        contextWindow: 8_192,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('docker-model-runner', [
        { tags: Array.from({ length: 6_000 }, (_, index) => `first/model-${String(index)}`) },
        { tags: Array.from({ length: 6_000 }, (_, index) => `second/model-${String(index)}`) },
      ]),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    await expect(
      listModelsFromMock('lemonade', {
        data: [
          { id: 'chat-model', labels: ['chat'] },
          { id: 'embed-model', labels: ['embeddings'] },
          { id: 'rerank-model', labels: ['reranking'] },
        ],
      }),
    ).resolves.toEqual([
      { id: 'chat-model', name: 'chat:chat-model', contextWindow: 8_192, vision: 'unknown' },
    ]);
    await expect(
      listModelsFromMock('privatemode', {
        data: [
          { id: 'private-chat', tasks: ['generate'] },
          { id: 'legacy/private-chat', tasks: ['generate'] },
          { id: 'private-embed', tasks: ['embed'] },
        ],
      }),
    ).resolves.toEqual([
      {
        id: 'private-chat',
        name: 'Private Chat',
        contextWindow: 16_384,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('togetherai', {
        data: [
          { id: 'together-chat', type: 'chat' },
          { id: 'together-not-chat', type: 'non-chat' },
          { id: 'together-chatx', type: 'chatx-tools' },
        ],
      }),
    ).resolves.toEqual([
      {
        id: 'together-chat',
        name: 'together-chat',
        contextWindow: 4_096,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('apipie', {
        data: [
          {
            provider: 'router',
            model: 'chat-model',
            type: 'non-chat',
            subtype: 'tool-chat-v2',
            max_tokens: 65_536,
          },
          { provider: 'router', model: 'other-model', subtype: 'text' },
        ],
      }),
    ).resolves.toEqual([
      {
        id: 'router/chat-model',
        name: 'router/chat-model',
        contextWindow: 65_536,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('cerebras', {
        data: [{ id: 'cerebras-chat', limits: { max_context_length: 131_072 } }],
      }),
    ).resolves.toEqual([
      {
        id: 'cerebras-chat',
        name: 'cerebras-chat',
        contextWindow: 131_072,
        vision: 'unknown',
      },
    ]);
    await expect(
      listModelsFromMock('openai', {
        data: [
          { id: 'gpt-4.1-nano', owned_by: 'openai' },
          { id: 'text-embedding-3-small', owned_by: 'openai' },
          { id: 'customer-fine-tune', owned_by: 'customer' },
        ],
      }),
    ).resolves.toEqual([
      {
        id: 'gpt-4.1-nano',
        name: 'gpt-4.1-nano',
        contextWindow: 1_047_576,
        vision: 'supported',
      },
      {
        id: 'customer-fine-tune',
        name: 'customer-fine-tune',
        contextWindow: 4_096,
        vision: 'unknown',
      },
    ]);
  });

  it('lists public models without credentials but requires one for connection/completion', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, { data: [{ id: 'public-model' }] }),
    );
    servers.push(server);
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { openrouter: server.origin },
    }).get('openrouter');
    const invocation = { config: { providerId: 'openrouter' as const }, credential: null };
    await expect(provider.listModels(invocation, AbortSignal.timeout(2_000))).resolves.toHaveLength(
      1,
    );
    expect(server.requests[0]?.headers.authorization).toBeUndefined();
    await expect(provider.validate(invocation, AbortSignal.timeout(2_000))).rejects.toMatchObject({
      code: 'MISSING_CREDENTIAL',
    });
    await expect(
      provider.cleanTranscript(invocation, { input: 'raw' }, AbortSignal.timeout(2_000)),
    ).rejects.toMatchObject({ code: 'MISSING_CREDENTIAL' });
  });

  it('validates authentication, reachability, and configured model presence', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.method === 'GET') {
        sendJson(response, { data: [{ id: 'available-model' }] });
        return;
      }
      sendJson(response, { error: 'invalid credential' }, 401);
    });
    servers.push(server);
    const registry = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: {
        perplexity: server.origin,
        openrouter: server.origin,
        'generic-openai': server.origin,
      },
    });
    for (const id of ['perplexity', 'openrouter'] as const) {
      await expect(
        registry.get(id).validate(
          {
            config: {
              providerId: id,
              modelId: id === 'perplexity' ? 'sonar' : 'available-model',
            },
            credential: 'invalid-key',
          },
          AbortSignal.timeout(2_000),
        ),
      ).rejects.toMatchObject({ code: 'AUTHENTICATION_FAILED' });
    }
    await expect(
      registry.get('generic-openai').validate(
        {
          config: {
            providerId: 'generic-openai',
            baseUrl: server.origin,
            modelId: 'missing-model',
          },
          credential: null,
        },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'MODEL_NOT_FOUND' });

    const stopped = await startMockProviderServer((_request, response) => sendJson(response, {}));
    const stoppedOrigin = stopped.origin;
    await stopped.close();
    const unavailable = new ProviderRegistry({
      endpointOverrides: { textgenwebui: stoppedOrigin },
    }).get('textgenwebui');
    await expect(
      unavailable.validate(
        {
          config: {
            providerId: 'textgenwebui',
            baseUrl: stoppedOrigin,
            modelId: 'gpt-test',
          },
          credential: null,
        },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE' });
  });

  it('maps remote status failures without exposing response bodies', async () => {
    const server = await startMockProviderServer((_request, response) => {
      sendJson(response, { secret: 'vendor body' }, 401);
    });
    servers.push(server);
    const transport = new PinnedJsonTransport();
    const error = await transport
      .request({
        url: server.origin,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      })
      .catch((failure: unknown) => failure);
    expect(error).toMatchObject({ code: 'AUTHENTICATION_FAILED' });
    expect(String(error)).not.toMatch(/vendor body|127\.0\.0\.1/);
  });
});

async function listModelsFromMock(
  id: OpenAICompatibleProviderId,
  body: unknown,
): Promise<readonly unknown[]> {
  const server = await startMockProviderServer((_request, response) => sendJson(response, body));
  servers.push(server);
  const preset = OPENAI_COMPATIBLE_PRESETS.find((candidate) => candidate.id === id);
  if (preset === undefined) throw new Error('preset is missing');
  const provider = new ProviderRegistry({
    transport: new PinnedJsonTransport(),
    endpointOverrides: { [id]: server.origin },
  }).get(id);
  return provider.listModels(
    {
      config: {
        providerId: id,
        ...(preset.endpoint.kind === 'configurable' ? { baseUrl: server.origin } : {}),
      },
      credential: preset.auth === 'none' ? null : 'test-key',
    },
    AbortSignal.timeout(2_000),
  );
}

async function captureCompletionPaths(origin: string): Promise<Record<string, string>> {
  const paths: Record<string, string> = {};
  for (const id of ['docker-model-runner', 'lemonade', 'lmstudio'] as const) {
    const before = activeServer(origin).requests.length;
    const provider = new ProviderRegistry({
      transport: new PinnedJsonTransport(),
      endpointOverrides: { [id]: origin },
    }).get(id);
    await provider.cleanTranscript(
      {
        config: { providerId: id, baseUrl: origin, modelId: 'gpt-test' },
        credential: id === 'lemonade' ? 'optional' : null,
      },
      { input: 'raw', maxOutputTokens: 100, temperature: 0.2 },
      AbortSignal.timeout(2_000),
    );
    paths[id] =
      activeServer(origin)
        .requests.slice(before)
        .find((request) => request.url.endsWith('/chat/completions'))?.url ?? '';
  }
  return paths;
}

function activeServer(origin: string): MockProviderServer {
  const server = servers.find((candidate) => candidate.origin === origin);
  if (server === undefined) throw new Error('mock server is missing');
  return server;
}

function contractConfig(providerId: (typeof PROVIDER_IDS)[number]): ProviderConfig {
  return {
    providerId,
    modelId: providerId === 'pi' ? 'contract/model' : 'contract-model',
    ...(CONFIGURABLE_PROVIDER_IDS.includes(providerId as never)
      ? {
          baseUrl:
            providerId === 'azure' ? 'https://contract.openai.azure.com' : 'http://127.0.0.1:43210',
        }
      : {}),
    ...(providerId === 'bedrock' ? { region: 'us-west-2' as const } : {}),
    ...(providerId === 'pi' ? { thinking: 'off' as const } : {}),
  };
}

function piContractOptions(blockCompletion = false) {
  return {
    platform: 'linux' as const,
    resolveCli: () => Promise.resolve(piIdentity('/canonical/pi')),
    spawnPi: (_executable: string, args: readonly string[]) => {
      const child = new EventEmitter() as ChildProcessWithoutNullStreams;
      const stdin = new PassThrough();
      const stdout = new PassThrough();
      const stderr = new PassThrough();
      Object.assign(child, { stdin, stdout, stderr, kill: () => true });
      if (args.includes('--list-models')) {
        setTimeout(() => {
          stdout.end(
            'provider  model           context  max-out  thinking  images\ncontract  model           8K       1K      no        no\n',
          );
          child.emit('close', 0, null);
        }, 0);
      } else if (!blockCompletion) {
        stdin.once('finish', () => {
          stdout.end('TALKING_QUILL_CONNECTION_OK');
          child.emit('close', 0, null);
        });
      }
      return child;
    },
  };
}

function piIdentity(path: string): PiCliIdentity {
  return {
    canonicalPath: path,
    packageVersion: 'future-compatible',
    safetyFlags: ['--no-tools', '--no-session', '--no-context-files', '--no-approve'],
    fileIdentity: { dev: '1', ino: '1', size: 1, mtimeMs: 1 },
  };
}

function blockingTransport(): JsonTransport {
  const rejectForMode = (request: JsonTransportRequest) =>
    new Promise<never>((_resolve, reject) => {
      if (request.signal.aborted) {
        reject(new ProviderError('CANCELLED'));
        return;
      }
      request.signal.addEventListener('abort', () => reject(new ProviderError('CANCELLED')), {
        once: true,
      });
    });
  return {
    request: rejectForMode,
    classify: (_url, _options, signal) =>
      rejectForMode({
        url: 'http://127.0.0.1:43210',
        method: 'GET',
        credentialed: false,
        signal,
      }),
  };
}
