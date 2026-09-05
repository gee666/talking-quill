import { PassThrough, Writable } from 'node:stream';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HelperRpcChannel } from '../../app/src/main/helper/helper-rpc-channel';
import { HelperClientError } from '../../app/src/main/helper/helper-client-error';
import { encodeHelperFrame, HelperFrameDecoder } from '../../app/src/main/helper/framing';
import { shortcutFromLegacyActivation } from '../../app/src/shared/schemas/shortcut';

const requestOptions = {
  timeoutMs: 1_000,
  timeoutReason: 'request-timeout',
  allowDraining: false,
  supervision: true,
} as const;
const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  vi.useRealTimers();
});

function createChannel() {
  const onFault = vi.fn();
  const onNotification = vi.fn();
  const channel = new HelperRpcChannel({
    createError: (code, message, rpcCode) => new HelperClientError(code, message, rpcCode),
    onFault,
    onNotification,
  });
  const attach = () => {
    const stdin = new Writable();
    const stdout = new PassThrough();
    const decoder = new HelperFrameDecoder();
    const requests: { id: number; method: string; params: unknown }[] = [];
    let blocked = false;
    vi.spyOn(stdin, 'write').mockImplementation((chunk: Uint8Array | string) => {
      for (const payload of decoder.push(Buffer.from(chunk))) {
        requests.push(JSON.parse(payload.toString('utf8')) as (typeof requests)[number]);
      }
      return !blocked;
    });
    const session = channel.attach({ stdin, stdout });
    cleanups.push(() => channel.close(session, new Error('Test cleanup')));
    return {
      session,
      stdin,
      stdout,
      requests,
      blockWrites: () => {
        blocked = true;
      },
      respond: (index: number, result: unknown) => {
        const request = requests[index];
        if (request === undefined) throw new Error('No dispatched request at index');
        stdout.emit('data', encodeHelperFrame({ jsonrpc: '2.0', id: request.id, result }));
      },
    };
  };
  return { channel, onFault, onNotification, attach, ...attach() };
}

describe('HelperRpcChannel extraction regressions', () => {
  it('keeps runtime operations out of the channel API', () => {
    const { channel } = createChannel();
    expect(Reflect.ownKeys(channel)).toEqual([]);
    expect(Object.getOwnPropertyNames(HelperRpcChannel.prototype)).toEqual([
      'constructor',
      'attach',
      'isCurrent',
      'resetOwnerActivationStream',
      'beginDraining',
      'request',
      'close',
    ]);
  });

  it('requires both a correlated response and drain before dispatching a backpressured successor', async () => {
    const controlled = createChannel();
    const { channel, session, requests, respond, stdin, onFault } = controlled;
    controlled.blockWrites();
    const first = channel.request(session, 'session.set_capture', { mode: 'off' }, requestOptions);
    const second = channel.request(
      session,
      'session.set_capture',
      { mode: 'recording' },
      requestOptions,
    );
    expect(requests).toHaveLength(1);
    respond(0, { mode: 'off' });
    await expect(first).resolves.toEqual({ mode: 'off' });
    expect(requests).toHaveLength(1);
    stdin.emit('drain');
    expect(requests).toHaveLength(2);
    respond(1, { mode: 'recording' });
    await expect(second).resolves.toEqual({ mode: 'recording' });
    expect(onFault).not.toHaveBeenCalled();
  });

  it('holds queued mutations after a malformed response until the supervisor admits shutdown', async () => {
    const { channel, session, requests, respond, onFault } = createChannel();
    const first = channel
      .request(session, 'session.set_capture', { mode: 'off' }, requestOptions)
      .catch((error: unknown) => error);
    const queued = channel
      .request(session, 'session.set_capture', { mode: 'recording' }, requestOptions)
      .catch((error: unknown) => error);
    respond(0, { mode: 'invalid' });
    expect(onFault).toHaveBeenCalledExactlyOnceWith(
      session,
      'malformed-response',
      expect.any(Error),
    );
    expect(requests).toHaveLength(1);
    await expect(first).resolves.toMatchObject({ code: 'transport-error' });

    channel.beginDraining(session);
    await expect(queued).resolves.toMatchObject({ code: 'not-running' });
    const shutdown = channel.request(
      session,
      'shutdown',
      {},
      {
        ...requestOptions,
        allowDraining: true,
        timeoutStartsOnDispatch: true,
      },
    );
    expect(requests.map((request) => request.method)).toEqual(['session.set_capture', 'shutdown']);
    respond(1, { ownerDisposition: 'neutral' });
    await expect(shutdown).resolves.toEqual({ ownerDisposition: 'neutral' });
  });

  it('starts the shutdown deadline on dispatch and ignores a drained predecessor response', async () => {
    vi.useFakeTimers();
    const { channel, session, requests, respond, onFault } = createChannel();
    const first = channel
      .request(
        session,
        'session.set_capture',
        { mode: 'off' },
        {
          ...requestOptions,
          timeoutMs: 20,
        },
      )
      .catch((error: unknown) => error);
    channel.beginDraining(session);
    const shutdown = channel.request(
      session,
      'shutdown',
      {},
      {
        ...requestOptions,
        timeoutMs: 10,
        allowDraining: true,
        timeoutStartsOnDispatch: true,
      },
    );
    await vi.advanceTimersByTimeAsync(15);
    expect(requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(5);
    await expect(first).resolves.toMatchObject({ code: 'request-timeout' });
    expect(requests).toHaveLength(2);
    respond(0, { mode: 'off' });
    expect(onFault).not.toHaveBeenCalled();
    respond(1, { ownerDisposition: 'neutral' });
    await expect(shutdown).resolves.toEqual({ ownerDisposition: 'neutral' });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('removes queued aborts but retains dispatched authority until its response', async () => {
    const { channel, session, requests, respond, onFault } = createChannel();
    const firstAbort = new AbortController();
    const secondAbort = new AbortController();
    const first = channel.request(
      session,
      'session.set_capture',
      { mode: 'off' },
      {
        ...requestOptions,
        signal: firstAbort.signal,
      },
    );
    const second = channel
      .request(
        session,
        'session.set_capture',
        { mode: 'recording' },
        {
          ...requestOptions,
          signal: secondAbort.signal,
        },
      )
      .catch((error: unknown) => error);
    firstAbort.abort();
    secondAbort.abort();
    await expect(second).resolves.toMatchObject({ name: 'AbortError' });
    expect(requests).toHaveLength(1);
    respond(0, { mode: 'off' });
    await expect(first).resolves.toEqual({ mode: 'off' });
    expect(requests).toHaveLength(1);
    expect(onFault).not.toHaveBeenCalled();
  });

  it('fences old streams and close calls after attaching a replacement session', async () => {
    const old = createChannel();
    const pending = old.channel
      .request(old.session, 'session.set_capture', { mode: 'off' }, requestOptions)
      .catch((error: unknown) => error);
    const closed = new Error('Replaced');
    old.channel.close(old.session, closed);
    await expect(pending).resolves.toBe(closed);
    const replacement = old.attach();
    const current = old.channel.request(
      replacement.session,
      'session.set_capture',
      { mode: 'recording' },
      requestOptions,
    );
    expect(replacement.requests[0]?.id).toBe((old.requests[0]?.id ?? 0) + 1);
    old.respond(0, { mode: 'off' });
    old.stdin.emit('drain');
    old.stdin.emit('error', new Error('Old stream'));
    old.stdout.emit('end');
    old.channel.beginDraining(old.session);
    old.channel.close(old.session, closed);
    expect(old.channel.isCurrent(replacement.session)).toBe(true);
    expect(old.onFault).not.toHaveBeenCalled();
    replacement.respond(0, { mode: 'recording' });
    await expect(current).resolves.toEqual({ mode: 'recording' });
  });

  it('publishes replacement activation pairs only after the matching configuration result', async () => {
    const { channel, session, stdout, respond, onNotification, onFault } = createChannel();
    const binding = { profileId: 'general', shortcut: shortcutFromLegacyActivation('Q', false) };
    const emit = (phase: 'down' | 'up', activationGeneration: number) =>
      stdout.emit(
        'data',
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: { ...binding, phase, activationGeneration, targetToken: null },
        }),
      );
    emit('down', 1);
    const params = { enabled: true, bindings: [binding] };
    const configuration = channel.request(session, 'activation.configure', params, requestOptions);
    emit('down', 2);
    emit('up', 2);
    expect(onNotification).toHaveBeenCalledTimes(1);
    respond(0, params);
    await expect(configuration).resolves.toEqual(params);
    expect(onNotification).toHaveBeenCalledTimes(3);
    channel.resetOwnerActivationStream(session);
    emit('down', 1);
    emit('up', 1);
    expect(onNotification).toHaveBeenCalledTimes(5);
    expect(onFault).not.toHaveBeenCalled();
  });
});
