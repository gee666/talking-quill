import { afterEach, describe, expect, it } from 'vitest';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';
import type { ProviderConfig } from '../../app/src/shared/schemas/providers';
import {
  sendJson,
  startMockProviderServer,
  type MockProviderServer,
} from '../helpers/mock-provider-server';

const servers: MockProviderServer[] = [];
afterEach(async () => Promise.all(servers.splice(0).map((server) => server.close())));

const credential = 'native-test-key';
const awsCredential = serializeAwsCredentials({
  accessKeyId: 'AKIDEXAMPLE123456',
  secretAccessKey: 'wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY',
  sessionToken: 'session-token-example-123456',
});

async function fixture() {
  const server = await startMockProviderServer((request, response) => {
    if (request.url.startsWith('/v1/models') && request.headers['x-api-key']) {
      sendJson(response, {
        data: [{ id: 'claude-sonnet-4-6', type: 'model', display_name: 'Claude Sonnet 4.6' }],
        has_more: false,
      });
    } else if (request.url === '/v1/messages') {
      sendJson(response, {
        type: 'message',
        role: 'assistant',
        stop_reason: 'end_turn',
        content: [{ type: 'text', text: 'anthropic clean' }],
      });
    } else if (request.url.startsWith('/v1beta/models?')) {
      sendJson(response, {
        models: [
          {
            name: 'models/gemini-2.0-flash-lite',
            displayName: 'Gemini Flash',
            inputTokenLimit: 1_000_000,
            supportedInputModalities: ['TEXT', 'IMAGE'],
            supportedGenerationMethods: ['generateContent'],
          },
        ],
      });
    } else if (request.url.includes(':generateContent')) {
      sendJson(response, {
        candidates: [
          { finishReason: 'STOP', content: { role: 'model', parts: [{ text: 'gemini clean' }] } },
        ],
      });
    } else if (request.url.includes('/openai/deployments/azure-deployment/chat/completions')) {
      sendJson(response, {
        choices: [
          { finish_reason: 'stop', message: { role: 'assistant', content: 'azure clean' } },
        ],
      });
    } else if (request.url.startsWith('/foundation-models')) {
      sendJson(response, {
        modelSummaries: [
          {
            modelId: 'anthropic.claude-3-sonnet',
            modelName: 'Claude 3 Sonnet',
            inputModalities: ['TEXT', 'IMAGE'],
            outputModalities: ['TEXT'],
            inferenceTypesSupported: ['ON_DEMAND'],
          },
        ],
      });
    } else if (request.url.startsWith('/inference-profiles')) {
      sendJson(response, {
        inferenceProfileSummaries: [
          {
            inferenceProfileId: 'us.anthropic.claude-3-sonnet-20240229-v1:0',
            inferenceProfileName: 'US Claude 3 Sonnet',
            inferenceProfileArn:
              'arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/profile-id',
            status: 'ACTIVE',
            models: [
              {
                modelArn: 'arn:aws:bedrock:us-east-1::foundation-model/anthropic.claude-3-sonnet',
              },
            ],
          },
        ],
      });
    } else if (request.url.includes('/converse')) {
      sendJson(response, {
        stopReason: 'end_turn',
        output: { message: { role: 'assistant', content: [{ text: 'bedrock clean' }] } },
      });
    } else if (request.url.startsWith('/v1/models?')) {
      sendJson(response, {
        models: [
          {
            name: 'command-a-03-2025',
            endpoints: ['chat'],
            features: ['vision'],
            context_length: 131_072,
          },
        ],
      });
    } else if (request.url === '/v2/chat') {
      sendJson(response, {
        finish_reason: 'COMPLETE',
        message: { role: 'assistant', content: [{ type: 'text', text: 'cohere clean' }] },
      });
    } else sendJson(response, { error: 'unexpected request' }, 404);
  });
  servers.push(server);
  const endpointOverrides = Object.fromEntries(
    ['anthropic', 'gemini', 'azure', 'bedrock', 'cohere'].map((id) => [id, server.origin]),
  );
  const service = new ProviderService(new ProviderRegistry({ endpointOverrides }), {
    getCredential: (providerId) => (providerId === 'bedrock' ? awsCredential : credential),
  });
  return { server, service };
}

const configs = [
  { providerId: 'anthropic', modelId: 'claude-sonnet-4-6' },
  { providerId: 'gemini', modelId: 'gemini-2.0-flash-lite' },
  {
    providerId: 'azure',
    baseUrl: 'https://fixture.openai.azure.com',
    modelId: 'azure-deployment',
    modelType: 'default',
  },
  {
    providerId: 'bedrock',
    region: 'us-west-2',
    modelId: 'us.anthropic.claude-3-sonnet-20240229-v1:0',
  },
  { providerId: 'cohere', modelId: 'command-a-03-2025' },
] as const satisfies readonly ProviderConfig[];

const expectedText: Readonly<Record<(typeof configs)[number]['providerId'], string>> = {
  anthropic: 'anthropic clean',
  gemini: 'gemini clean',
  azure: 'azure clean',
  bedrock: 'bedrock clean',
  cohere: 'cohere clean',
};

describe('native cloud provider contracts', () => {
  it('lists, validates, completes, classifies, and reports vision for all five adapters', async () => {
    const { service } = await fixture();
    for (const config of configs) {
      const models = await service.listModels(config, AbortSignal.timeout(2_000));
      expect(
        models.some((model) => model.id === config.modelId),
        config.providerId,
      ).toBe(true);
      const result = await service.testConnection(config, AbortSignal.timeout(2_000));
      expect(result).toMatchObject({ ok: true, destination: 'local' });
      await expect(
        service.cleanTranscript(
          config,
          { input: 'raw transcript', maxOutputTokens: 32 },
          AbortSignal.timeout(2_000),
        ),
        config.providerId,
      ).resolves.toBe(expectedText[config.providerId]);
      const vision = service.capabilities(config, config.modelId);
      expect(vision, config.providerId).toBe(
        config.providerId === 'azure' ? 'unknown' : 'supported',
      );
    }
  });

  it('preflights cold catalog-backed vision capabilities and caches completed discovery', async () => {
    const { server, service } = await fixture();
    for (const config of configs.filter(
      (candidate) => candidate.providerId === 'bedrock' || candidate.providerId === 'cohere',
    )) {
      expect(service.capabilities(config, config.modelId)).toBe('unknown');
      await expect(
        service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
      ).resolves.toBe('supported');
      const requestCount = server.requests.length;
      await expect(
        service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
      ).resolves.toBe('supported');
      expect(server.requests).toHaveLength(requestCount);
    }
  });

  it('does not reuse catalog-backed capability caches across credentials', async () => {
    let cohereCredential = 'credential-account-a';
    let bedrockCredential = awsCredential;
    const secondAwsCredential = serializeAwsCredentials({
      accessKeyId: 'AKIDACCOUNTB123456',
      secretAccessKey: 'second-account-secret-access-key',
    });
    const server = await startMockProviderServer((request, response) => {
      if (request.url.startsWith('/v1/models?')) {
        sendJson(response, {
          models: [
            {
              name: 'account-model',
              endpoints: ['chat'],
              features:
                request.headers.authorization === 'Bearer credential-account-a'
                  ? ['vision']
                  : ['chat'],
            },
          ],
        });
      } else if (request.url.startsWith('/foundation-models')) {
        sendJson(response, {
          modelSummaries: [
            {
              modelId: 'account-model',
              inputModalities: request.headers.authorization?.includes('AKIDACCOUNTB123456')
                ? ['TEXT']
                : ['TEXT', 'IMAGE'],
              outputModalities: ['TEXT'],
              inferenceTypesSupported: ['ON_DEMAND'],
            },
          ],
        });
      } else if (request.url.startsWith('/inference-profiles')) {
        sendJson(response, { inferenceProfileSummaries: [] });
      } else {
        sendJson(response, {}, 404);
      }
    });
    servers.push(server);
    const service = new ProviderService(
      new ProviderRegistry({
        endpointOverrides: { bedrock: server.origin, cohere: server.origin },
      }),
      {
        getCredential: (providerId) =>
          providerId === 'bedrock' ? bedrockCredential : cohereCredential,
      },
    );
    const cohereConfig = { providerId: 'cohere' as const, modelId: 'account-model' };
    const bedrockConfig = {
      providerId: 'bedrock' as const,
      region: 'us-west-2',
      modelId: 'account-model',
    };

    await expect(
      service.preflightCapability(cohereConfig, cohereConfig.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('supported');
    cohereCredential = 'credential-account-b';
    await expect(
      service.preflightCapability(cohereConfig, cohereConfig.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('unsupported');

    await expect(
      service.preflightCapability(bedrockConfig, bedrockConfig.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('supported');
    bedrockCredential = secondAwsCredential;
    await expect(
      service.preflightCapability(bedrockConfig, bedrockConfig.modelId, AbortSignal.timeout(2_000)),
    ).resolves.toBe('unsupported');
  });

  it('does not publish Cohere capabilities from a failed paginated discovery', async () => {
    const server = await startMockProviderServer((request, response) => {
      const url = new URL(request.url, server.origin);
      if (url.searchParams.get('page_token') === null) {
        sendJson(response, {
          models: [
            {
              name: 'partially-discovered-model',
              endpoints: ['chat'],
              features: ['vision'],
            },
          ],
          next_page_token: 'next',
        });
      } else {
        sendJson(response, { models: 'malformed' });
      }
    });
    servers.push(server);
    const service = new ProviderService(
      new ProviderRegistry({ endpointOverrides: { cohere: server.origin } }),
      { getCredential: () => credential },
    );
    const config = { providerId: 'cohere' as const, modelId: 'partially-discovered-model' };

    await expect(
      service.preflightCapability(config, config.modelId, AbortSignal.timeout(2_000)),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    expect(service.capabilities(config, config.modelId)).toBe('unknown');
  });

  it('encodes optional images in each provider-native request shape', async () => {
    const { server, service } = await fixture();
    const image = { mimeType: 'image/jpeg' as const, base64: '/9j/2Q==' };
    for (const config of configs) {
      await service.cleanTranscript(
        config,
        { input: 'raw transcript', maxOutputTokens: 32, image },
        AbortSignal.timeout(2_000),
      );
      const body = server.requests.at(-1)?.body;
      if (config.providerId === 'anthropic') {
        expect(body).toMatchObject({
          messages: [{ content: [{ type: 'text' }, { source: { data: image.base64 } }] }],
        });
      } else if (config.providerId === 'gemini') {
        expect(body).toMatchObject({
          contents: [
            { parts: [{ text: 'raw transcript' }, { inlineData: { data: image.base64 } }] },
          ],
        });
      } else if (config.providerId === 'bedrock') {
        expect(body).toMatchObject({
          messages: [
            {
              content: [{ text: 'raw transcript' }, { image: { source: { bytes: image.base64 } } }],
            },
          ],
        });
      } else {
        expect(body).toMatchObject({
          messages: [
            {
              content: [
                { type: 'text', text: 'raw transcript' },
                {
                  type: 'image_url',
                  image_url: { url: `data:image/jpeg;base64,${image.base64}` },
                },
              ],
            },
          ],
        });
      }
    }
  });

  it('uses Gemini input modality metadata conservatively', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, {
        models: [
          {
            name: 'models/gemini-custom',
            supportedInputModalities: ['TEXT', 'IMAGE'],
            supportedGenerationMethods: ['generateContent'],
          },
          {
            name: 'models/gemini-2.0-text',
            supportedInputModalities: ['TEXT'],
            supportedGenerationMethods: ['generateContent'],
          },
          {
            name: 'models/gemini-unclassified',
            supportedGenerationMethods: ['generateContent'],
          },
        ],
      }),
    );
    servers.push(server);
    const provider = new ProviderService(
      new ProviderRegistry({ endpointOverrides: { gemini: server.origin } }),
      { getCredential: () => credential },
    );
    await expect(
      provider.listModels(
        { providerId: 'gemini', modelId: 'gemini-custom' },
        AbortSignal.timeout(2_000),
      ),
    ).resolves.toEqual([
      { id: 'gemini-custom', name: 'gemini-custom', contextWindow: null, vision: 'supported' },
      {
        id: 'gemini-2.0-text',
        name: 'gemini-2.0-text',
        contextWindow: null,
        vision: 'unsupported',
      },
      {
        id: 'gemini-unclassified',
        name: 'gemini-unclassified',
        contextWindow: null,
        vision: 'unknown',
      },
    ]);
  });

  it('treats Azure deployment selection as explicit configuration, not remote discovery', async () => {
    const { server, service } = await fixture();
    await expect(service.listModels(configs[2], AbortSignal.timeout(2_000))).resolves.toEqual([
      {
        id: 'azure-deployment',
        name: 'azure-deployment',
        contextWindow: null,
        vision: 'unknown',
      },
    ]);
    expect(server.requests).toHaveLength(0);
    expect(new ProviderRegistry().catalog().find(({ id }) => id === 'azure')?.modelDiscovery).toBe(
      'azure-deployment',
    );
  });

  it('uses native paths, protocol headers, safe queries, and non-streaming request shapes', async () => {
    const { server, service } = await fixture();
    for (const config of configs) {
      await service.listModels(config, AbortSignal.timeout(2_000));
      await service.cleanTranscript(
        config,
        { input: 'private transcript', temperature: 0.2, maxOutputTokens: 64 },
        AbortSignal.timeout(2_000),
      );
    }
    const source = JSON.stringify(server.requests);
    expect(source).not.toContain(`${credential}?`);
    expect(
      server.requests.find((request) => request.url === '/v1/messages')?.headers,
    ).toMatchObject({ 'x-api-key': credential, 'anthropic-version': '2023-06-01' });
    expect(
      server.requests.find((request) => request.url.includes(':generateContent'))?.headers[
        'x-goog-api-key'
      ],
    ).toBe(credential);
    expect(
      server.requests.find((request) => request.url.includes('api-version=2024-10-21'))?.headers[
        'api-key'
      ],
    ).toBe(credential);
    const bedrockCompletion = server.requests.find((request) => request.url.includes('/converse'));
    expect(bedrockCompletion?.headers.authorization).toMatch(
      /^AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE123456\//,
    );
    expect(bedrockCompletion?.headers.authorization).not.toContain('Bearer');
    expect(bedrockCompletion?.headers['x-amz-date']).toMatch(/^\d{8}T\d{6}Z$/);
    expect(bedrockCompletion?.headers['x-amz-content-sha256']).toMatch(/^[a-f0-9]{64}$/);
    expect(bedrockCompletion?.headers['x-amz-security-token']).toBe('session-token-example-123456');
    for (const request of server.requests.filter(
      ({ url }) =>
        url.startsWith('/foundation-models') ||
        url.startsWith('/inference-profiles') ||
        url.includes('/converse'),
    )) {
      expect(request.headers.authorization).toMatch(/^AWS4-HMAC-SHA256 /);
      expect(request.headers.authorization).not.toContain('Bearer');
    }
    expect(bedrockCompletion?.body).toMatchObject({
      inferenceConfig: { temperature: 0.2, maxTokens: 64 },
    });
    expect(bedrockCompletion?.url).toContain(
      '/model/us.anthropic.claude-3-sonnet-20240229-v1%3A0/converse',
    );
    expect(server.requests.find((request) => request.url === '/v2/chat')?.body).toMatchObject({
      stream: false,
      max_tokens: 64,
    });
  });

  it('validates a configured Bedrock application inference-profile ARN alias', async () => {
    const { server, service } = await fixture();
    const profileArn =
      'arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/profile-id';
    const config = {
      providerId: 'bedrock' as const,
      region: 'us-west-2',
      modelId: profileArn,
    };
    await expect(service.testConnection(config, AbortSignal.timeout(2_000))).resolves.toMatchObject(
      { ok: true, modelCount: 3 },
    );
    expect(service.capabilities(config, profileArn)).toBe('supported');
    expect(server.requests.at(-1)?.url).toContain(
      'arn%3Aaws%3Abedrock%3Aus-west-2%3A123456789012%3Aapplication-inference-profile%2Fprofile-id',
    );
  });

  it('cancels Bedrock sibling catalog pagination before returning a branch failure', async () => {
    let foundationRequests = 0;
    const server = await startMockProviderServer((request, response) => {
      if (request.url.startsWith('/foundation-models')) {
        foundationRequests += 1;
        setTimeout(() => {
          if (!response.destroyed) {
            sendJson(response, { modelSummaries: [], nextToken: 'another-page' });
          }
        }, 100);
        return;
      }
      if (request.url.startsWith('/inference-profiles')) {
        sendJson(response, { inferenceProfileSummaries: 'malformed' });
        return;
      }
      sendJson(response, {}, 404);
    });
    servers.push(server);
    const service = new ProviderService(
      new ProviderRegistry({ endpointOverrides: { bedrock: server.origin } }),
      { getCredential: () => awsCredential },
    );

    await expect(
      service.listModels(
        { providerId: 'bedrock', region: 'us-west-2', modelId: 'profile-id' },
        AbortSignal.timeout(2_000),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(foundationRequests).toBe(1);
  });

  it('rejects malformed Bedrock inference-profile ARN aliases', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url.startsWith('/foundation-models')) {
        sendJson(response, {
          modelSummaries: [
            {
              modelId: 'foundation-before-failure',
              inputModalities: ['TEXT', 'IMAGE'],
              outputModalities: ['TEXT'],
              inferenceTypesSupported: ['ON_DEMAND'],
            },
          ],
        });
        return;
      }
      sendJson(response, {
        inferenceProfileSummaries: [
          {
            inferenceProfileId: 'profile-id',
            inferenceProfileArn: 'arn:aws:bedrock:us-west-2:123456789012:role/not-a-profile',
            status: 'ACTIVE',
            models: [],
          },
        ],
      });
    });
    servers.push(server);
    const service = new ProviderService(
      new ProviderRegistry({ endpointOverrides: { bedrock: server.origin } }),
      { getCredential: () => awsCredential },
    );
    const config = { providerId: 'bedrock' as const, region: 'us-west-2', modelId: 'profile-id' };
    await expect(service.listModels(config, AbortSignal.timeout(2_000))).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
    });
    expect(service.capabilities(config, 'foundation-before-failure')).toBe('unknown');
  });

  it('omits Bedrock temperature for incompatible models while retaining output bounds', async () => {
    const { server, service } = await fixture();
    const config = {
      providerId: 'bedrock' as const,
      region: 'us-west-2',
      modelId: 'us.anthropic.claude-opus-4-7-v1:0',
    };
    await service.cleanTranscript(
      config,
      { input: 'raw', temperature: 0.2, maxOutputTokens: 64 },
      AbortSignal.timeout(2_000),
    );
    const request = server.requests.find((candidate) => candidate.url.includes('claude-opus-4-7'));
    expect(request?.body).toMatchObject({ inferenceConfig: { maxTokens: 64 } });
    expect((request?.body as { inferenceConfig?: object }).inferenceConfig).not.toHaveProperty(
      'temperature',
    );

    const profileArn =
      'arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/profile-id';
    await service.cleanTranscript(
      { ...config, modelId: profileArn },
      { input: 'raw' },
      AbortSignal.timeout(2_000),
    );
    expect(server.requests.at(-1)?.url).toContain(
      'arn%3Aaws%3Abedrock%3Aus-west-2%3A123456789012%3Aapplication-inference-profile%2Fprofile-id',
    );
  });

  it('rejects incomplete native responses and never returns partial text', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, {
        type: 'message',
        role: 'assistant',
        stop_reason: 'max_tokens',
        content: [{ type: 'text', text: 'partial' }],
      }),
    );
    servers.push(server);
    const service = new ProviderService(
      new ProviderRegistry({ endpointOverrides: { anthropic: server.origin } }),
      { getCredential: () => credential },
    );
    await expect(
      service.cleanTranscript(configs[0], { input: 'raw' }, AbortSignal.timeout(2_000)),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });
});
