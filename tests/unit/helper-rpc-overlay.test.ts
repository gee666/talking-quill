import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { PassThrough, Writable } from 'node:stream';
import { runInNewContext } from 'node:vm';
import { build } from 'vite';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { helperResultSchemas } from '../../app/src/shared/helper/protocol';
import { transformWindowsInstalledAcceptanceSource } from '../../app/windows-installed-acceptance-overlay';
import {
  type HelperRpcChannel,
  type HelperRpcSession,
} from '../../app/src/main/helper/helper-rpc-channel';
import { type HelperRpcRequestOptions } from '../../app/src/main/helper/helper-rpc-runtime';
import { encodeHelperFrame, HelperFrameDecoder } from '../../app/src/main/helper/framing';

type AcceptanceChannel = HelperRpcChannel & {
  requestAcceptance(
    session: HelperRpcSession,
    method: string,
    resultSchema: (typeof helperResultSchemas)['session.set_capture'],
    options: HelperRpcRequestOptions,
  ): Promise<unknown>;
};

let Channel: new (...args: ConstructorParameters<typeof HelperRpcChannel>) => AcceptanceChannel;
const cleanups: (() => void)[] = [];
const options = {
  timeoutMs: 1_000,
  timeoutReason: 'request-timeout',
  allowDraining: false,
  supervision: false,
} as const;

beforeAll(async () => {
  const entry = resolve('app/src/main/helper/helper-rpc-channel.ts');
  const output = await build({
    configFile: false,
    mode: 'production',
    logLevel: 'silent',
    plugins: [
      {
        name: 'helper-channel-acceptance-regression',
        enforce: 'pre',
        transform(source, id) {
          if (id.replaceAll('\\', '/') !== entry.replaceAll('\\', '/')) return null;
          return transformWindowsInstalledAcceptanceSource('helperChannel', source);
        },
      },
    ],
    build: {
      ssr: entry,
      write: false,
      rollupOptions: { output: { format: 'cjs' } },
    },
  });
  if (Array.isArray(output) || !('output' in output)) throw new Error('Expected one bundle');
  const chunk = output.output.find((item) => item.type === 'chunk' && item.isEntry);
  if (chunk?.type !== 'chunk') throw new Error('Expected a bundled entry');
  const exports: { HelperRpcChannel?: typeof Channel } = {};
  runInNewContext(chunk.code, {
    exports,
    module: { exports },
    require: createRequire(entry),
    Buffer,
    DOMException,
    TextDecoder,
    TextEncoder,
    setTimeout,
    clearTimeout,
  });
  if (exports.HelperRpcChannel === undefined) throw new Error('Channel export is missing');
  Channel = exports.HelperRpcChannel;
});

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
});

function createChannel() {
  const onFault = vi.fn();
  const channel = new Channel({
    createError: (code, message) => Object.assign(new Error(message), { code }),
    onFault,
    onNotification: vi.fn(),
  });
  const stdout = new PassThrough();
  const decoder = new HelperFrameDecoder();
  const requests: { id: number; method: string; params: unknown }[] = [];
  const stdin = new Writable({
    write(chunk: Buffer, _encoding, callback) {
      for (const payload of decoder.push(chunk)) {
        requests.push(JSON.parse(payload.toString('utf8')) as (typeof requests)[number]);
      }
      callback();
    },
  });
  const session = channel.attach({ stdin, stdout });
  cleanups.push(() => channel.close(session, new Error('Test cleanup')));
  return {
    channel,
    session,
    requests,
    onFault,
    respond: (index: number, result: unknown) => {
      const request = requests[index];
      if (request === undefined) throw new Error('No dispatched request');
      stdout.write(encodeHelperFrame({ jsonrpc: '2.0', id: request.id, result }));
    },
  };
}

describe('production acceptance helper channel overlay', () => {
  it('uses the supplied result schema and the same serialized transport as typed requests', async () => {
    const { channel, session, requests, respond, onFault } = createChannel();
    const acceptance = channel.requestAcceptance(
      session,
      'acceptance.probe',
      helperResultSchemas['session.set_capture'],
      options,
    );
    const ordinary = channel.request(session, 'session.set_capture', { mode: 'off' }, options);
    expect(requests).toEqual([{ jsonrpc: '2.0', id: 1, method: 'acceptance.probe', params: {} }]);
    respond(0, { mode: 'off' });
    await expect(acceptance).resolves.toEqual({ mode: 'off' });
    expect(requests[1]?.method).toBe('session.set_capture');
    respond(1, { mode: 'off' });
    await expect(ordinary).resolves.toEqual({ mode: 'off' });
    expect(onFault).not.toHaveBeenCalled();
  });

  it('rejects malformed acceptance results without releasing queued mutations', async () => {
    const { channel, session, requests, respond, onFault } = createChannel();
    const acceptance = channel
      .requestAcceptance(
        session,
        'acceptance.probe',
        helperResultSchemas['session.set_capture'],
        options,
      )
      .catch((error: unknown) => error);
    const ordinary = channel
      .request(session, 'session.set_capture', { mode: 'off' }, options)
      .catch((error: unknown) => error);
    respond(0, { mode: 'invalid' });
    await expect(acceptance).resolves.toMatchObject({ code: 'transport-error' });
    expect(onFault).toHaveBeenCalledOnce();
    expect(onFault.mock.calls[0]?.[1]).toBe('malformed-response');
    expect(requests).toHaveLength(1);
    channel.beginDraining(session);
    await expect(ordinary).resolves.toMatchObject({ code: 'not-running' });
    await expect(
      channel.requestAcceptance(
        session,
        'acceptance.probe',
        helperResultSchemas['session.set_capture'],
        options,
      ),
    ).rejects.toMatchObject({ code: 'not-running' });
  });
});
