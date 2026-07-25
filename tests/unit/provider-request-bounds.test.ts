import { describe, expect, it, vi } from 'vitest';
import type { JsonTransport } from '../../app/src/main/providers/json-transport';
import {
  MAX_PROVIDER_REQUEST_BYTES,
  PinnedJsonTransport,
} from '../../app/src/main/providers/json-transport';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import {
  MAX_PROVIDER_INPUT_UTF8_BYTES,
  ProviderCompletionRequestSchema,
} from '../../app/src/shared/schemas/providers';
import { sendJson, startMockProviderServer } from '../helpers/mock-provider-server';

describe('provider request byte bounds', () => {
  it('validates ASCII and multibyte input at the exact UTF-8 boundary', () => {
    const ascii = 'a'.repeat(MAX_PROVIDER_INPUT_UTF8_BYTES);
    const multibyte = '😀'.repeat(MAX_PROVIDER_INPUT_UTF8_BYTES / 4);
    expect(ProviderCompletionRequestSchema.safeParse({ input: ascii }).success).toBe(true);
    expect(ProviderCompletionRequestSchema.safeParse({ input: multibyte }).success).toBe(true);
    expect(ProviderCompletionRequestSchema.safeParse({ input: `${ascii}a` }).success).toBe(false);
    expect(ProviderCompletionRequestSchema.safeParse({ input: `${multibyte}a` }).success).toBe(
      false,
    );
  });

  it('rejects oversized multibyte transcripts before provider or credential access', async () => {
    const request = vi.fn<JsonTransport['request']>();
    const getCredential = vi.fn(() => 'unused-credential');
    const service = new ProviderService(
      new ProviderRegistry({
        transport: { request, classify: () => Promise.resolve('local') },
      }),
      { getCredential },
    );
    await expect(
      service.cleanTranscript(
        { providerId: 'openai', modelId: 'gpt-4.1-nano' },
        { input: `${'😀'.repeat(MAX_PROVIDER_INPUT_UTF8_BYTES / 4)}a` },
        new AbortController().signal,
      ),
    ).rejects.toMatchObject({ code: 'REQUEST_TOO_LARGE' });
    expect(getCredential).not.toHaveBeenCalled();
    expect(request).not.toHaveBeenCalled();
  });

  it('rejects JSON escaping expansion beyond the 512 KiB wire cap without network I/O', async () => {
    const transport = new PinnedJsonTransport();
    await expect(
      transport.request({
        url: 'http://127.0.0.1:1',
        method: 'POST',
        body: { input: '"'.repeat(300_000) },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'REQUEST_TOO_LARGE' });
  });

  it.each([
    [
      'OpenAI Responses',
      (text: string) => ({
        model: 'm',
        input: [
          {
            role: 'user',
            content: [
              { type: 'input_text', text },
              { type: 'input_image', image_url: 'data:image/jpeg;base64,/9j/2Q==' },
            ],
          },
        ],
      }),
    ],
    [
      'OpenAI chat',
      (text: string) => ({
        model: 'm',
        messages: [
          {
            role: 'user',
            content: [
              { type: 'text', text },
              { type: 'image_url', image_url: { url: 'data:image/jpeg;base64,/9j/2Q==' } },
            ],
          },
        ],
      }),
    ],
    [
      'Ollama',
      (text: string) => ({
        model: 'm',
        messages: [{ role: 'user', content: text, images: ['/9j/2Q=='] }],
      }),
    ],
    [
      'Anthropic',
      (text: string) => ({
        model: 'm',
        messages: [
          {
            role: 'user',
            content: [
              { type: 'text', text },
              {
                type: 'image',
                source: { type: 'base64', media_type: 'image/jpeg', data: '/9j/2Q==' },
              },
            ],
          },
        ],
      }),
    ],
    [
      'Gemini',
      (text: string) => ({
        contents: [
          {
            role: 'user',
            parts: [{ text }, { inlineData: { mimeType: 'image/jpeg', data: '/9j/2Q==' } }],
          },
        ],
      }),
    ],
    [
      'Azure',
      (text: string) => ({
        messages: [
          {
            role: 'user',
            content: [
              { type: 'text', text },
              { type: 'image_url', image_url: { url: 'data:image/jpeg;base64,/9j/2Q==' } },
            ],
          },
        ],
      }),
    ],
    [
      'Bedrock',
      (text: string) => ({
        messages: [
          {
            role: 'user',
            content: [{ text }, { image: { format: 'jpeg', source: { bytes: '/9j/2Q==' } } }],
          },
        ],
      }),
    ],
    ['Cohere', (text: string) => ({ model: 'm', messages: [{ role: 'user', content: text }] })],
  ] as const)(
    'enforces the combined serialized %s prompt/image/JSON budget',
    async (_name, body) => {
      const server = await startMockProviderServer((_request, response) => sendJson(response, {}));
      try {
        const transport = new PinnedJsonTransport();
        const overhead = Buffer.byteLength(JSON.stringify(body('')), 'utf8');
        const atLimit = body('a'.repeat(MAX_PROVIDER_REQUEST_BYTES - overhead));
        await expect(
          transport.request({
            url: server.origin,
            method: 'POST',
            body: atLimit,
            credentialed: false,
            signal: AbortSignal.timeout(2_000),
          }),
        ).resolves.toBeDefined();
        await expect(
          transport.request({
            url: server.origin,
            method: 'POST',
            body: body('a'.repeat(MAX_PROVIDER_REQUEST_BYTES - overhead + 1)),
            credentialed: false,
            signal: AbortSignal.timeout(2_000),
          }),
        ).rejects.toMatchObject({ code: 'REQUEST_TOO_LARGE' });
      } finally {
        await server.close();
      }
    },
  );
});
