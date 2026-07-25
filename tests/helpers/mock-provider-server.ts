import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

export interface CapturedRequest {
  readonly method: string;
  readonly url: string;
  readonly headers: Readonly<Record<string, string | readonly string[] | undefined>>;
  readonly body: unknown;
}

export type MockProviderHandler = (
  request: CapturedRequest,
  response: ServerResponse,
) => void | Promise<void>;

export interface MockProviderServer {
  readonly origin: string;
  readonly requests: CapturedRequest[];
  close(): Promise<void>;
}

export async function startMockProviderServer(
  handler: MockProviderHandler,
): Promise<MockProviderServer> {
  const requests: CapturedRequest[] = [];
  const server = createServer((incoming, response) => {
    void capture(incoming)
      .then(async (request) => {
        requests.push(request);
        await handler(request, response);
      })
      .catch(() => {
        response.writeHead(400, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ error: 'invalid request' }));
      });
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });
  const address = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${String(address.port)}`,
    requests,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.close((error) => (error === undefined ? resolve() : reject(error)));
        server.closeAllConnections();
      }),
  };
}

export function sendJson(
  response: ServerResponse,
  body: unknown,
  status = 200,
  headers: Readonly<Record<string, string>> = {},
): void {
  const source = JSON.stringify(body);
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': String(Buffer.byteLength(source)),
    ...headers,
  });
  response.end(source);
}

async function capture(incoming: IncomingMessage): Promise<CapturedRequest> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of incoming) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as Uint8Array);
    size += bytes.length;
    if (size > 1024 * 1024) throw new Error('request too large');
    chunks.push(bytes);
  }
  const source = Buffer.concat(chunks).toString('utf8');
  let body: unknown = null;
  if (source.length > 0) body = JSON.parse(source) as unknown;
  return Object.freeze({
    method: incoming.method ?? '',
    url: incoming.url ?? '',
    headers: Object.freeze({ ...incoming.headers }),
    body,
  });
}
