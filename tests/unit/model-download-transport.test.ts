import { describe, expect, it, vi } from 'vitest';
import { ModelDownloadTransport } from '../../app/src/main/transcription/model-download-transport';
import type { ModelManifestEntry } from '../../app/src/shared/schemas/model-manifest';

const model = {
  id: 'Xenova/whisper-small',
  revision: 'a'.repeat(40),
  dtype: 'q8',
  totalBytes: 6,
  files: [{ path: 'model.bin', size: 6, sha256: '0'.repeat(64) }],
} as unknown as ModelManifestEntry;
const file = model.files[0];
if (file === undefined) throw new Error('Fixture missing');

describe('ModelDownloadTransport', () => {
  it('retains range headers across trusted redirects and observes each contacted hop', async () => {
    const requests: { readonly url: string; readonly headers: Headers }[] = [];
    const fetchModel = vi.fn<typeof fetch>((input, init) => {
      const url = input instanceof Request ? input.url : String(input);
      requests.push({ url, headers: new Headers(init?.headers) });
      return Promise.resolve(
        url.endsWith('/start')
          ? new Response(null, { status: 302, headers: { location: '/final' } })
          : new Response('def'),
      );
    });
    const egress: string[] = [];
    const transport = new ModelDownloadTransport({
      fetch: fetchModel,
      urlFor: () => 'https://trusted.example/start',
      validateRequestUrl: (url) => url.startsWith('https://trusted.example/'),
      observeEgress: (category) => egress.push(category),
    });

    const response = await transport.request(model, file, 3, new AbortController().signal);

    expect(requests.map(({ url }) => url)).toEqual([
      'https://trusted.example/start',
      'https://trusted.example/final',
    ]);
    for (const request of requests) {
      expect(request.headers.get('accept-encoding')).toBe('identity');
      expect(request.headers.get('range')).toBe('bytes=3-');
    }
    expect(egress).toEqual(['model-download', 'model-download']);
    expect(await response.read()).toMatchObject({
      done: false,
      value: new Uint8Array([100, 101, 102]),
    });
    expect(await response.read()).toEqual({ done: true, value: undefined });
  });

  it('cancels a redirect body and refuses an untrusted destination before contact', async () => {
    let redirectCancelled = false;
    const contacted: string[] = [];
    const transport = new ModelDownloadTransport({
      fetch: (input) => {
        contacted.push(input instanceof Request ? input.url : String(input));
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              cancel() {
                redirectCancelled = true;
              },
            }),
            { status: 302, headers: { location: 'https://untrusted.invalid/model' } },
          ),
        );
      },
      urlFor: () => 'https://trusted.example/start',
      validateRequestUrl: (url) => url.startsWith('https://trusted.example/'),
    });

    await expect(
      transport.request(model, file, 0, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'PROTOCOL' });
    expect(contacted).toEqual(['https://trusted.example/start']);
    expect(redirectCancelled).toBe(true);
  });
});
