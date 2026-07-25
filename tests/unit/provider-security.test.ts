import { afterEach, describe, expect, it, vi } from 'vitest';
import { PinnedJsonTransport } from '../../app/src/main/providers/json-transport';
import {
  ProviderError,
  providerErrorFromStatus,
  toProviderError,
} from '../../app/src/main/providers/errors';
import { normalizeBaseUrl } from '../../app/src/main/providers/openai-compatible';
import {
  classifyIpAddress,
  validateProviderEndpoint,
  type EndpointResolver,
} from '../../app/src/main/security/provider-endpoint-policy';
import { redactHeaders, redactSensitive, redactText } from '../../app/src/main/security/redaction';
import {
  sendJson,
  startMockProviderServer,
  type MockProviderServer,
} from '../helpers/mock-provider-server';

const servers: MockProviderServer[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
});

const liveSignal = (): AbortSignal => new AbortController().signal;

describe('provider endpoint policy', () => {
  it.each([
    ['127.0.0.1', 'local'],
    ['::1', 'local'],
    ['::ffff:127.0.0.1', 'local'],
    ['10.1.2.3', 'lan'],
    ['172.20.1.2', 'lan'],
    ['192.168.2.3', 'lan'],
    ['fd12::1', 'lan'],
    ['8.8.8.8', 'cloud'],
    ['2606:4700:4700::1111', 'cloud'],
    ['169.254.169.254', 'blocked'],
    ['168.63.129.16', 'blocked'],
    ['100.100.100.200', 'blocked'],
    ['0.0.0.0', 'blocked'],
    ['224.0.0.1', 'blocked'],
    ['240.0.0.1', 'blocked'],
    ['fe80::1', 'blocked'],
    ['fec0::1', 'blocked'],
    ['2001:db8::1', 'blocked'],
    ['fd00:ec2::254', 'blocked'],
    ['fd00:ec2::23', 'blocked'],
    ['::169.254.169.254', 'blocked'],
    ['64:ff9b::a9fe:a9fe', 'blocked'],
    ['2002:a9fe:a9fe::1', 'blocked'],
    ['3fff::1', 'blocked'],
  ])('classifies %s as %s', (address, expected) => {
    expect(classifyIpAddress(address)).toBe(expected);
  });

  it('blocks metadata, alternate numeric forms, unsafe URL components, and mixed DNS answers', async () => {
    const options = { credentialed: false, signal: liveSignal() } as const;
    await expect(
      validateProviderEndpoint('http://169.254.169.254', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://2852039166', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://[fd00:ec2::254]', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://user:pass@127.0.0.1', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://127.0.0.1/?api_key=secret', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://127.0.0.1/?key=secret', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    expect(() =>
      normalizeBaseUrl('http://127.0.0.1:8000/v1?key=persisted-secret', 'preserve'),
    ).toThrow(expect.objectContaining({ code: 'INVALID_CONFIG' }));
    await expect(
      validateProviderEndpoint('file:///tmp/provider', undefined, options),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint(
        'https://mixed.test',
        () =>
          Promise.resolve([
            { address: '127.0.0.1', family: 4 },
            { address: '8.8.8.8', family: 4 },
          ]),
        options,
      ),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
  });

  it('allows cleartext credentials only on loopback and constrains fixed cloud endpoints', async () => {
    const loopback = await validateProviderEndpoint('http://127.0.0.1:1234', undefined, {
      credentialed: true,
      signal: liveSignal(),
    });
    expect(loopback.destination).toBe('local');
    await expect(
      validateProviderEndpoint('http://lan.test', resolverFor('192.168.1.10'), {
        credentialed: true,
        signal: liveSignal(),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://cloud.test', resolverFor('8.8.8.8'), {
        credentialed: true,
        signal: liveSignal(),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('http://cloud.test', resolverFor('8.8.8.8'), {
        credentialed: false,
        fixedCloud: true,
        signal: liveSignal(),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('https://private.test', resolverFor('10.0.0.2'), {
        credentialed: false,
        fixedCloud: true,
        signal: liveSignal(),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      validateProviderEndpoint('https://cloud.test', resolverFor('8.8.8.8'), {
        credentialed: true,
        fixedCloud: true,
        signal: liveSignal(),
      }),
    ).resolves.toMatchObject({ destination: 'cloud' });
  });
});

describe('socket-pinned JSON transport', () => {
  it('pins a validated hostname address into the Node 24 socket lookup without second DNS', async () => {
    const server = await startMockProviderServer((request, response) => {
      expect(request.headers.host).toMatch(/^pin\.invalid:/);
      sendJson(response, { ok: true });
    });
    servers.push(server);
    const resolver = vi
      .fn<EndpointResolver>()
      .mockResolvedValue([{ address: '127.0.0.1', family: 4 }]);
    const transport = new PinnedJsonTransport(resolver);
    const port = new URL(server.origin).port;
    await expect(
      transport.request({
        url: `http://pin.invalid:${port}/json`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ body: { ok: true }, destination: 'local' });
    expect(resolver).toHaveBeenCalledTimes(1);
  });

  it('reuses one operation-scoped DNS pin across logical requests', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, { ok: true }),
    );
    servers.push(server);
    const resolver = vi
      .fn<EndpointResolver>()
      .mockResolvedValueOnce([{ address: '127.0.0.1', family: 4 }])
      .mockResolvedValueOnce([{ address: '169.254.169.254', family: 4 }]);
    const transport = new PinnedJsonTransport(resolver);
    const signal = AbortSignal.timeout(2_000);
    const port = new URL(server.origin).port;
    const endpoint = `http://alternating.invalid:${port}`;

    await expect(
      Promise.all([
        transport.request({
          url: `${endpoint}/models`,
          method: 'GET',
          credentialed: false,
          signal,
        }),
        transport.request({
          url: `${endpoint}/details`,
          method: 'GET',
          credentialed: false,
          signal,
        }),
      ]),
    ).resolves.toHaveLength(2);
    expect(resolver).toHaveBeenCalledTimes(1);
  });

  it('enforces cumulative response bytes and retries only validated GET addresses', async () => {
    const server = await startMockProviderServer((_request, response) =>
      sendJson(response, { value: 'x'.repeat(40) }),
    );
    servers.push(server);
    const port = new URL(server.origin).port;
    const retryResolver = vi.fn<EndpointResolver>().mockResolvedValue([
      { address: '127.0.0.2', family: 4 },
      { address: '127.0.0.1', family: 4 },
    ]);
    const retryTransport = new PinnedJsonTransport(retryResolver);
    await expect(
      retryTransport.request({
        url: `http://retry.invalid:${port}`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ destination: 'local' });
    expect(retryResolver).toHaveBeenCalledTimes(1);

    const transport = new PinnedJsonTransport();
    const signal = AbortSignal.timeout(2_000);
    await transport.request({
      url: server.origin,
      method: 'GET',
      credentialed: false,
      signal,
      maxOperationResponseBytes: 80,
    });
    await expect(
      transport.request({
        url: server.origin,
        method: 'GET',
        credentialed: false,
        signal,
        maxOperationResponseBytes: 80,
      }),
    ).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });
  });

  it('covers timeout and cancellation while an injected DNS resolver is pending', async () => {
    const neverResolves: EndpointResolver = () => new Promise(() => undefined);
    const transport = new PinnedJsonTransport(neverResolves);
    await expect(
      transport.request({
        url: 'http://pending.invalid',
        method: 'GET',
        credentialed: false,
        signal: liveSignal(),
        timeoutMs: 25,
      }),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });

    const controller = new AbortController();
    const pending = transport.request({
      url: 'http://pending.invalid',
      method: 'GET',
      credentialed: false,
      signal: controller.signal,
      timeoutMs: 2_000,
    });
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED' });
  });

  it('reuses the original pin on same-origin redirects and revalidates other origins', async () => {
    let origin = '';
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/same') {
        response.writeHead(307, { location: '/final' });
        response.end();
      } else if (request.url === '/cross') {
        const port = new URL(origin).port;
        response.writeHead(307, { location: `http://localhost:${port}/final` });
        response.end();
      } else if (request.url === '/metadata') {
        response.writeHead(307, { location: 'http://169.254.169.254/latest' });
        response.end();
      } else {
        sendJson(response, { redirected: true });
      }
    });
    servers.push(server);
    origin = server.origin;
    const port = new URL(origin).port;
    const resolver = vi
      .fn<EndpointResolver>()
      .mockResolvedValueOnce([{ address: '127.0.0.1', family: 4 }])
      .mockResolvedValueOnce([{ address: '169.254.169.254', family: 4 }]);
    const pinned = new PinnedJsonTransport(resolver);
    await expect(
      pinned.request({
        url: `http://rebind.invalid:${port}/same`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ body: { redirected: true } });
    expect(resolver).toHaveBeenCalledTimes(1);

    const transport = new PinnedJsonTransport();
    await expect(
      transport.request({
        url: `${origin}/cross`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ body: { redirected: true }, destination: 'local' });
    await expect(
      transport.request({
        url: `${origin}/cross`,
        method: 'GET',
        headers: { authorization: 'Bearer secret' },
        credentialed: true,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      transport.request({
        url: `${origin}/cross`,
        method: 'POST',
        body: { transcript: 'must-not-be-replayed' },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
    await expect(
      transport.request({
        url: `${origin}/metadata`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
  });

  it('stops after at most three followed redirects', async () => {
    const server = await startMockProviderServer((request, response) => {
      const index = Number(request.url.slice(2));
      response.writeHead(307, { location: `/r${String(index + 1)}` });
      response.end();
    });
    servers.push(server);
    await expect(
      new PinnedJsonTransport().request({
        url: `${server.origin}/r0`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'REMOTE_FAILURE' });
    expect(server.requests).toHaveLength(4);
  });

  it('applies safe redirect method semantics and preserves POST only for 307/308', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/see-other') {
        response.writeHead(303, { location: '/captured' });
        response.end();
      } else if (request.url === '/temporary') {
        response.writeHead(307, { location: '/captured' });
        response.end();
      } else if (request.url === '/ambiguous') {
        response.writeHead(302, { location: '/captured' });
        response.end();
      } else {
        sendJson(response, { method: request.method, body: request.body });
      }
    });
    servers.push(server);
    const transport = new PinnedJsonTransport();
    await expect(
      transport.request({
        url: `${server.origin}/see-other`,
        method: 'POST',
        body: { transcript: 'private' },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ body: { method: 'GET', body: null } });
    await expect(
      transport.request({
        url: `${server.origin}/temporary`,
        method: 'POST',
        body: { value: 'safe' },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).resolves.toMatchObject({ body: { method: 'POST', body: { value: 'safe' } } });
    await expect(
      transport.request({
        url: `${server.origin}/ambiguous`,
        method: 'POST',
        body: { value: 'blocked' },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'SECURITY_BLOCKED' });
  });

  it('distinguishes response timeouts and caller cancellation', async () => {
    const server = await startMockProviderServer((_request, response) => {
      setTimeout(() => {
        if (!response.destroyed) sendJson(response, { late: true });
      }, 500);
    });
    servers.push(server);
    const transport = new PinnedJsonTransport();
    await expect(
      transport.request({
        url: server.origin,
        method: 'GET',
        credentialed: false,
        signal: liveSignal(),
        timeoutMs: 25,
      }),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });

    const controller = new AbortController();
    const pending = transport.request({
      url: server.origin,
      method: 'GET',
      credentialed: false,
      signal: controller.signal,
      timeoutMs: 2_000,
    });
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED' });
  });

  it('bounds declared and streamed response sizes and rejects malformed JSON', async () => {
    const server = await startMockProviderServer((request, response) => {
      if (request.url === '/declared') {
        response.writeHead(200, { 'content-type': 'application/json', 'content-length': '9999' });
        response.end('{}');
      } else if (request.url === '/streamed') {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ value: 'x'.repeat(2_000) }));
      } else {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end('{');
      }
    });
    servers.push(server);
    const transport = new PinnedJsonTransport();
    await expect(
      transport.request({
        url: server.origin,
        method: 'GET',
        body: { unexpected: true },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    await expect(
      transport.request({
        url: server.origin,
        method: 'POST',
        body: { value: 'x'.repeat(600 * 1_024) },
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'REQUEST_TOO_LARGE' });
    expect(server.requests).toHaveLength(0);
    for (const path of ['/declared', '/streamed']) {
      await expect(
        transport.request({
          url: `${server.origin}${path}`,
          method: 'GET',
          credentialed: false,
          signal: AbortSignal.timeout(2_000),
          maxResponseBytes: 100,
        }),
      ).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });
    }
    await expect(
      transport.request({
        url: `${server.origin}/malformed`,
        method: 'GET',
        credentialed: false,
        signal: AbortSignal.timeout(2_000),
      }),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });
});

describe('safe errors and redaction', () => {
  it.each([
    [401, 'AUTHENTICATION_FAILED'],
    [403, 'AUTHENTICATION_FAILED'],
    [404, 'MODEL_NOT_FOUND'],
    [408, 'TIMEOUT'],
    [413, 'RESPONSE_TOO_LARGE'],
    [429, 'RATE_LIMITED'],
    [500, 'UNAVAILABLE'],
    [400, 'REMOTE_FAILURE'],
  ])('maps HTTP %s to %s', (status, code) => {
    expect(providerErrorFromStatus(status)).toMatchObject({ code });
  });

  it('maps public failures to fixed messages containing no remote data', () => {
    const secret = 'sk-secret-value';
    for (const error of [
      providerErrorFromStatus(401),
      providerErrorFromStatus(429),
      new ProviderError('UNAVAILABLE'),
      toProviderError(new Error('vendor failure at https://secret.example with sk-secret-value')),
    ]) {
      const publicError = error.toPublicError();
      expect(publicError.message).not.toMatch(/https?:|example|127\.|vendor|authorization/i);
      expect(JSON.stringify(publicError)).not.toContain(secret);
    }
  });

  it('deeply redacts URLs, hosts, addresses, headers, bodies, explicit secrets, and IPv6', () => {
    const secret = 'sk-secret-value';
    const value = redactSensitive(
      {
        endpoint: 'https://provider.example/v1?key=visible',
        authorization: `Bearer ${secret}`,
        nested: { body: 'vendor response', address: '169.254.169.254' },
        hostname: 'internal-provider',
      },
      [secret],
    );
    const source = JSON.stringify(value);
    expect(source).not.toMatch(
      /provider\.example|internal-provider|169\.254|vendor response|sk-secret|bearer/i,
    );
    expect(
      redactText(`failed at (fd00:ec2::254), api.example from 8.8.8.8 using ${secret}`, [secret]),
    ).not.toMatch(/fd00:ec2|api\.example|8\.8\.8\.8|sk-secret/);
    expect(redactHeaders({ authorization: `Bearer ${secret}`, accept: 'api.example' })).toEqual({
      authorization: '[REDACTED]',
      accept: '[REDACTED]',
    });
    expect(
      JSON.stringify(redactSensitive(new Error('getaddrinfo ENOTFOUND internal-provider'))),
    ).not.toContain('internal-provider');
  });
});

function resolverFor(address: string): EndpointResolver {
  return () => Promise.resolve([{ address, family: address.includes(':') ? 6 : 4 }]);
}
