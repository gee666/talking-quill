import { describe, expect, it, vi } from 'vitest';
import type { JsonTransport } from '../../app/src/main/providers/json-transport';
import { ProviderError } from '../../app/src/main/providers/errors';
import {
  compareVersions,
  normalizeVersion,
  UpdateService,
} from '../../app/src/main/info/update-service';

function transport(body: unknown): JsonTransport {
  return {
    request: vi.fn().mockResolvedValue({ body, destination: 'cloud', status: 200 }),
    classify: vi.fn(),
  };
}

describe('manual update checking', () => {
  it('parses stable tags and compares all numeric components', () => {
    expect(normalizeVersion('v1.2.30')).toBe('1.2.30');
    expect(compareVersions('1.10.0', '1.9.9')).toBe(1);
    expect(compareVersions('9007199254740993.0.0', '9007199254740992.999.999')).toBe(1);
    expect(compareVersions('1.0.0', '1.0.0')).toBe(0);
    expect(() => normalizeVersion('01.0.0')).toThrow();
    expect(() => normalizeVersion('1.0.0-beta.1')).toThrow();
  });

  it('reports available and current releases through the bounded fixed endpoint', async () => {
    const mock = transport({
      tag_name: 'v2.0.0',
      html_url: 'https://github.com/gee666/talking-quill/releases/tag/v2.0.0',
      draft: false,
      prerelease: false,
    });
    const service = new UpdateService(mock);
    await expect(service.check('1.9.0', new AbortController().signal)).resolves.toMatchObject({
      status: 'available',
      latestVersion: '2.0.0',
    });
    // The transport mock intentionally exposes the contract method for request-shape inspection.
    // eslint-disable-next-line @typescript-eslint/unbound-method
    expect(mock.request).toHaveBeenCalledWith(
      expect.objectContaining({
        method: 'GET',
        fixedCloud: true,
        allowedOrigins: ['https://api.github.com'],
        credentialed: false,
        maxResponseBytes: 262144,
      }),
    );
    await expect(service.check('2.1.0', new AbortController().signal)).resolves.toMatchObject({
      status: 'current',
    });
  });

  it('rejects an invalid current version before starting a network request', async () => {
    const mock = transport({});
    await expect(
      new UpdateService(mock).check('1.0.0\r\nInjected: value', new AbortController().signal),
    ).rejects.toThrow('release version was invalid');
    // eslint-disable-next-line @typescript-eslint/unbound-method
    expect(mock.request).not.toHaveBeenCalled();
  });

  it('rejects prereleases, malformed versions, and non-GitHub release links', async () => {
    for (const body of [
      {
        tag_name: 'v2.0.0-beta',
        html_url: 'https://github.com/gee666/talking-quill/releases/tag/test',
        draft: false,
        prerelease: false,
      },
      {
        tag_name: 'v2.0.0',
        html_url: 'https://example.com/release',
        draft: false,
        prerelease: false,
      },
      {
        tag_name: 'v2.0.0',
        html_url: 'https://github.com/gee666/talking-quill/releases/tag/v2.0.1',
        draft: false,
        prerelease: false,
      },
      {
        tag_name: 'v2.0.0',
        html_url: 'https://github.com/gee666/talking-quill/releases/tag/%762.0.0',
        draft: false,
        prerelease: false,
      },
      {
        tag_name: 'v2.0.0',
        html_url: 'https://github.com/gee666/talking-quill/releases/tag/test',
        draft: false,
        prerelease: true,
      },
      {
        tag_name: 'v2.0.0',
        html_url: 'https://github.com/gee666/talking-quill/releases/tag/test',
        draft: true,
        prerelease: false,
      },
    ])
      await expect(
        new UpdateService(transport(body)).check('1.0.0', new AbortController().signal),
      ).rejects.toThrow();
  });

  it('maps a missing or unavailable fixed repository without leaking transport detail', async () => {
    const mock: JsonTransport = {
      request: vi.fn().mockRejectedValue(new ProviderError('MODEL_NOT_FOUND')),
      classify: vi.fn(),
    };
    await expect(
      new UpdateService(mock).check('1.0.0', new AbortController().signal),
    ).rejects.toThrow('No public GitHub release exists for gee666/talking-quill.');
  });

  it('passes cancellation to the transport and performs no background request', async () => {
    const request = vi.fn<JsonTransport['request']>(
      (options) =>
        new Promise((_resolve, reject) =>
          options.signal.addEventListener('abort', () => reject(new Error('cancelled')), {
            once: true,
          }),
        ),
    );
    const mock: JsonTransport = { request, classify: vi.fn() };
    const controller = new AbortController();
    const pending = new UpdateService(mock).check('1.0.0', controller.signal);
    expect(request).toHaveBeenCalledOnce();
    controller.abort();
    await expect(pending).rejects.toThrow('cancelled');
  });
});
