import { afterEach, describe, expect, it } from 'vitest';
import { ProviderError } from '../../app/src/main/providers/errors';
import type { JsonTransport } from '../../app/src/main/providers/json-transport';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';
import type { NativeCloudProviderId, ProviderConfig } from '../../app/src/shared/schemas/providers';
import {
  sendJson,
  startMockProviderServer,
  type MockProviderServer,
} from '../helpers/mock-provider-server';

const credential = 'native-resilience-key';
const awsCredential = serializeAwsCredentials({
  accessKeyId: 'AKIDRESILIENCE1234',
  secretAccessKey: 'resilience-secret-access-key-example',
});
const servers: MockProviderServer[] = [];
afterEach(async () => Promise.all(servers.splice(0).map((server) => server.close())));

const cases = [
  { id: 'anthropic', config: { providerId: 'anthropic', modelId: 'claude-sonnet-4-6' } },
  { id: 'gemini', config: { providerId: 'gemini', modelId: 'gemini-2.0-flash-lite' } },
  {
    id: 'azure',
    config: {
      providerId: 'azure',
      baseUrl: 'https://fixture.openai.azure.com',
      modelId: 'deployment',
    },
  },
  {
    id: 'bedrock',
    config: { providerId: 'bedrock', region: 'us-west-2', modelId: 'profile:model' },
  },
  { id: 'cohere', config: { providerId: 'cohere', modelId: 'command-a-03-2025' } },
] as const satisfies readonly { id: NativeCloudProviderId; config: ProviderConfig }[];

describe('native provider resilience contract', () => {
  it.each(cases)('$id cancels an in-flight native request', async ({ id, config }) => {
    let requestStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      requestStarted = resolve;
    });
    const server = await startMockProviderServer(() => {
      requestStarted();
    });
    servers.push(server);
    const service = serviceFor(id, server.origin);
    const controller = new AbortController();
    const pending = service.cleanTranscript(config, { input: 'raw' }, controller.signal);
    await started;
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED' });
  });

  it.each(cases)('$id preserves timeout errors from the shared transport', async ({ config }) => {
    const transport: JsonTransport = {
      request: () => Promise.reject(new ProviderError('TIMEOUT')),
      classify: () => Promise.reject(new ProviderError('TIMEOUT')),
    };
    const service = new ProviderService(new ProviderRegistry({ transport }), {
      getCredential: (providerId) => (providerId === 'bedrock' ? awsCredential : credential),
    });
    await expect(
      service.cleanTranscript(config, { input: 'raw' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });
  });

  it.each([
    { status: 401, code: 'AUTHENTICATION_FAILED' },
    { status: 429, code: 'RATE_LIMITED' },
    { status: 500, code: 'UNAVAILABLE' },
  ] as const)('maps HTTP $status for every native completion', async ({ status, code }) => {
    for (const { id, config } of cases) {
      const server = await startMockProviderServer((_request, response) =>
        sendJson(response, { error: 'private vendor detail' }, status),
      );
      servers.push(server);
      await expect(
        serviceFor(id, server.origin).cleanTranscript(
          config,
          { input: 'raw' },
          AbortSignal.timeout(2_000),
        ),
        id,
      ).rejects.toMatchObject({ code });
    }
  });

  it('maps only bounded Gemini API_KEY_INVALID envelopes to authentication failure', async () => {
    const valid = await startMockProviderServer((_request, response) =>
      sendJson(
        response,
        {
          error: {
            code: 400,
            status: 'INVALID_ARGUMENT',
            message: 'API key not valid: must-not-leak',
            details: [
              {
                reason: 'API_KEY_INVALID',
                domain: 'googleapis.com',
                metadata: { key: credential },
              },
            ],
          },
        },
        400,
      ),
    );
    servers.push(valid);
    const gemini = cases.find(({ id }) => id === 'gemini');
    if (gemini === undefined) throw new Error('Gemini fixture missing');
    const authenticationError = await serviceFor('gemini', valid.origin)
      .cleanTranscript(gemini.config, { input: 'raw' }, AbortSignal.timeout(2_000))
      .catch((error: unknown) => error);
    expect(authenticationError).toMatchObject({ code: 'AUTHENTICATION_FAILED' });
    expect(String(authenticationError)).not.toMatch(/must-not-leak|native-resilience-key/);

    const unrelated = await startMockProviderServer((_request, response) =>
      sendJson(
        response,
        {
          error: {
            code: 400,
            status: 'INVALID_ARGUMENT',
            details: [{ reason: 'INVALID_PROMPT', domain: 'googleapis.com' }],
          },
        },
        400,
      ),
    );
    servers.push(unrelated);
    await expect(
      serviceFor('gemini', unrelated.origin).cleanTranscript(
        gemini.config,
        { input: 'raw' },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'REMOTE_FAILURE' });
  });

  it('does not decode oversized Gemini error bodies', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(
        response,
        {
          padding: 'x'.repeat(17 * 1_024),
          error: {
            status: 'INVALID_ARGUMENT',
            details: [{ reason: 'API_KEY_INVALID', domain: 'googleapis.com' }],
          },
        },
        400,
      ),
    );
    servers.push(server);
    const gemini = cases.find(({ id }) => id === 'gemini');
    if (gemini === undefined) throw new Error('Gemini fixture missing');
    await expect(
      serviceFor('gemini', server.origin).cleanTranscript(
        gemini.config,
        { input: 'raw' },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'REMOTE_FAILURE' });
  });

  it.each(cases)('$id rejects completion output beyond the adapter cap', async ({ id, config }) => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, oversizedCompletion(id)),
    );
    servers.push(server);
    await expect(
      serviceFor(id, server.origin).cleanTranscript(
        config,
        { input: 'raw' },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });

  it.each(cases.filter(({ id }) => id !== 'azure'))(
    '$id rejects oversized model collections',
    async ({ id, config }) => {
      const server = await startMockProviderServer((_request, response) =>
        sendJson(response, oversizedModelList(id)),
      );
      servers.push(server);
      await expect(
        serviceFor(id, server.origin).listModels(config, AbortSignal.timeout(2_000)),
      ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    },
  );

  it.each(cases.filter(({ id }) => id !== 'azure'))(
    '$id bounds malformed or endless pagination',
    async ({ id, config }) => {
      const server = await startMockProviderServer((_request, response) =>
        sendJson(response, paginatedModelList(id)),
      );
      servers.push(server);
      await expect(
        serviceFor(id, server.origin).listModels(config, AbortSignal.timeout(5_000)),
      ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
      expect(server.requests).toHaveLength(20);
    },
  );
});

function serviceFor(id: NativeCloudProviderId, origin: string): ProviderService {
  return new ProviderService(new ProviderRegistry({ endpointOverrides: { [id]: origin } }), {
    getCredential: (providerId) => (providerId === 'bedrock' ? awsCredential : credential),
  });
}

function oversizedCompletion(id: NativeCloudProviderId): unknown {
  const text = 'x'.repeat(200_001);
  if (id === 'anthropic')
    return {
      type: 'message',
      role: 'assistant',
      stop_reason: 'end_turn',
      content: [{ type: 'text', text }],
    };
  if (id === 'gemini')
    return {
      candidates: [{ finishReason: 'STOP', content: { role: 'model', parts: [{ text }] } }],
    };
  if (id === 'azure')
    return { choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: text } }] };
  if (id === 'bedrock')
    return {
      stopReason: 'end_turn',
      output: { message: { role: 'assistant', content: [{ text }] } },
    };
  return {
    finish_reason: 'COMPLETE',
    message: { role: 'assistant', content: [{ type: 'text', text }] },
  };
}

function oversizedModelList(id: NativeCloudProviderId): unknown {
  const items = Array.from({ length: 2_001 }, (_, index) => ({ id: `model-${String(index)}` }));
  if (id === 'anthropic') return { data: items, has_more: false };
  if (id === 'gemini') return { models: items };
  if (id === 'bedrock') return { modelSummaries: items };
  if (id === 'cohere') return { models: items };
  throw new Error('Azure uses configured deployment semantics');
}

function paginatedModelList(id: NativeCloudProviderId): unknown {
  if (id === 'anthropic')
    return { data: [{ id: 'claude-sonnet-4-6', type: 'model' }], has_more: true };
  if (id === 'gemini')
    return {
      models: [
        {
          name: 'models/gemini-2.0-flash-lite',
          supportedGenerationMethods: ['generateContent'],
        },
      ],
      nextPageToken: 'same-token',
    };
  if (id === 'bedrock')
    return {
      modelSummaries: [
        {
          modelId: 'model',
          inputModalities: ['TEXT'],
          outputModalities: ['TEXT'],
          inferenceTypesSupported: ['ON_DEMAND'],
        },
      ],
      nextToken: 'same-token',
    };
  if (id === 'cohere') {
    return {
      models: [{ name: 'command-a-03-2025', endpoints: ['chat'] }],
      next_page_token: 'same-token',
    };
  }
  throw new Error('Azure uses configured deployment semantics');
}
