import { describe, expect, it, vi } from 'vitest';
import { PinnedJsonTransport } from '../../app/src/main/providers/json-transport';
import type { EndpointResolver } from '../../app/src/main/security/provider-endpoint-policy';

const resolver = vi.fn<EndpointResolver>().mockResolvedValue([{ address: '8.8.8.8', family: 4 }]);

describe('manual update transport allowlist', () => {
  it('rejects a non-allowlisted initial origin before opening a socket', async () => {
    const transport = new PinnedJsonTransport(resolver);
    await expect(
      transport.request({
        url: 'https://example.com/releases/latest',
        method: 'GET',
        credentialed: false,
        fixedCloud: true,
        allowedOrigins: ['https://api.github.com'],
        signal: AbortSignal.timeout(1_000),
        timeoutMs: 500,
        maxResponseBytes: 1_024,
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
  });

  it('rejects insecure, credentialed, path-bearing, or empty allowlist entries', async () => {
    for (const allowedOrigins of [
      [],
      ['http://api.github.com'],
      ['https://user@api.github.com'],
      ['https://api.github.com/path'],
    ]) {
      await expect(
        new PinnedJsonTransport(resolver).request({
          url: 'https://api.github.com',
          method: 'GET',
          credentialed: false,
          fixedCloud: true,
          allowedOrigins,
          signal: AbortSignal.timeout(1_000),
        }),
      ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    }
  });
});
