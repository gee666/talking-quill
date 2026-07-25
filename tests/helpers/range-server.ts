import { createServer, type Server } from 'node:http';

export interface RangeFixture {
  readonly path: string;
  readonly body: Uint8Array;
  readonly ignoreRange?: boolean;
}

export interface RangeServer {
  readonly origin: string;
  readonly ranges: readonly string[];
  close(): Promise<void>;
}

export async function startRangeServer(fixtures: readonly RangeFixture[]): Promise<RangeServer> {
  const ranges: string[] = [];
  const byPath = new Map(fixtures.map((fixture) => [fixture.path, fixture]));
  const server = createServer((request, response) => {
    const fixture = byPath.get(request.url ?? '');
    if (fixture === undefined) {
      response.writeHead(404).end();
      return;
    }
    const { body } = fixture;
    const range = request.headers.range;
    if (range !== undefined) ranges.push(range);
    const match = /^bytes=(\d+)-$/.exec(range ?? '');
    const start = match === null || fixture.ignoreRange === true ? 0 : Number(match[1]);
    if (!Number.isSafeInteger(start) || start < 0 || start >= body.byteLength) {
      response.writeHead(416).end();
      return;
    }
    const chunk = body.subarray(start);
    response.writeHead(start === 0 ? 200 : 206, {
      'content-length': String(chunk.byteLength),
      ...(start === 0
        ? {}
        : {
            'content-range': `bytes ${String(start)}-${String(body.byteLength - 1)}/${String(body.byteLength)}`,
          }),
    });
    response.end(chunk);
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (typeof address !== 'object' || address === null)
    throw new Error('Range server did not bind.');
  return {
    origin: `http://127.0.0.1:${String(address.port)}`,
    ranges,
    close: () => closeServer(server),
  };
}

function closeServer(server: Server): Promise<void> {
  return new Promise((resolve, reject) => {
    server.close((error) => (error === undefined ? resolve() : reject(error)));
  });
}
