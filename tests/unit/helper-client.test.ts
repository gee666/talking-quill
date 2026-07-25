import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HelperClient } from '../../app/src/main/helper/helper-client';
import { encodeHelperFrame, HelperFrameDecoder } from '../../app/src/main/helper/framing';

const fixture = resolve('tests/fixtures/fake-helper.mjs');
const clients: HelperClient[] = [];

afterEach(async () => Promise.all(clients.splice(0).map((client) => client.stop())));

function createClient(scenario: string): HelperClient {
  const platform = process.platform === 'win32' ? 'win32' : 'darwin';
  const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    spawnHelper: (_path, options) =>
      spawn(process.execPath, [fixture, scenario], {
        ...options,
        stdio: ['pipe', 'pipe', 'pipe'],
      }),
  });
  clients.push(client);
  return client;
}

async function waitFor(predicate: () => boolean, timeoutMilliseconds = 4_000): Promise<void> {
  const deadline = Date.now() + timeoutMilliseconds;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('Timed out waiting for helper state');
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
}

function createControlledClient(): {
  client: HelperClient;
  writes: Buffer[];
  blockNextWrite: () => void;
  emitDrain: () => void;
  emitStdout: (chunk: Buffer) => void;
  endStdout: () => void;
  emitChildError: () => void;
  close: () => void;
  kill: ReturnType<typeof vi.fn>;
} {
  const stdin = new EventEmitter() as EventEmitter & {
    destroyed: boolean;
    writable: boolean;
    write: (frame: Buffer, callback?: (error?: Error | null) => void) => boolean;
  };
  const stdout = new EventEmitter();
  const stderr = new EventEmitter();
  stdin.destroyed = false;
  stdin.writable = true;
  const writes: Buffer[] = [];
  const decoder = new HelperFrameDecoder();
  let blockNext = false;
  const kill = vi.fn(() => true);

  stdin.write = (frame: Buffer): boolean => {
    writes.push(Buffer.from(frame));
    for (const payload of decoder.push(frame)) {
      const request = JSON.parse(payload.toString('utf8')) as { id: number; method: string };
      if (request.method === 'initialize') {
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: {
                protocolVersion: 2,
                helperVersion: '1.0.0',
                platform: process.platform === 'win32' ? 'windows' : 'macos',
                architecture: process.arch === 'arm64' ? 'aarch64' : 'x86_64',
                defaultActivationKey: 'Z',
                hookStatus: 'ready',
                permissions: {
                  accessibility: 'not_applicable',
                  inputMonitoring: 'not_applicable',
                  eventPost: 'not_applicable',
                },
              },
            }),
          );
        });
      }
    }
    if (!blockNext) return true;
    blockNext = false;
    return false;
  };

  const processEmitter = new EventEmitter() as EventEmitter & {
    stdin: typeof stdin;
    stdout: EventEmitter;
    stderr: EventEmitter;
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    kill: ReturnType<typeof vi.fn>;
  };
  processEmitter.stdin = stdin;
  processEmitter.stdout = stdout;
  processEmitter.stderr = stderr;
  processEmitter.exitCode = null;
  processEmitter.signalCode = null;
  processEmitter.kill = kill;
  const child = processEmitter as unknown as ChildProcessWithoutNullStreams;

  const platform = process.platform === 'win32' ? 'win32' : 'darwin';
  const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    spawnHelper: () => child,
  });
  clients.push(client);

  return {
    client,
    writes,
    blockNextWrite: () => {
      blockNext = true;
    },
    emitDrain: () => stdin.emit('drain'),
    emitStdout: (chunk) => stdout.emit('data', chunk),
    endStdout: () => stdout.emit('end'),
    emitChildError: () => processEmitter.emit('error', new Error('child failed')),
    close: () => {
      stdin.writable = false;
      processEmitter.exitCode = 1;
      processEmitter.emit('close', 1, null);
    },
    kill,
  };
}

describe('supervised native HelperClient', () => {
  it('handshakes, starts with activation disabled, correlates typed requests, and shuts down', async () => {
    const client = createClient('normal');
    const readiness: string[] = [];
    client.subscribeReadiness((value) => readiness.push(value.status));
    await client.start();

    expect(client.readiness).toMatchObject({ status: 'ready', helperVersion: '1.0.0' });
    expect(readiness).toContain('ready');
    await expect(
      client.configureActivation(true, [
        { key: 'Z', shift: false },
        { key: 'Z', shift: true },
      ]),
    ).resolves.toEqual({
      enabled: true,
      bindings: [
        { key: 'Z', shift: false },
        { key: 'Z', shift: true },
      ],
    });
    await expect(client.setSessionCapture(true)).resolves.toEqual({ active: true });
    await expect(
      Promise.all([client.getFrontApp(), client.injectPaste(), client.ping()]),
    ).resolves.toEqual([
      { processName: 'fixture-app', windowTitle: 'Fixture target', windowBounds: null },
      { submitted: true },
      { ok: true, hookStatus: 'ready' },
    ]);

    await client.stop();
    expect(client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
  });

  it('uses the protocol default without a redundant startup configuration round trip', async () => {
    const client = createClient('reject-default-config');
    await client.start();

    expect(client.readiness.status).toBe('ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('preserves the irreversible commit when abort precedes a delayed acknowledgement', async () => {
    const client = createClient('paste-delay');
    await client.start();
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = client.injectPaste(controller.signal, committed);
    controller.abort();
    await expect(paste).resolves.toEqual({ submitted: true });
    expect(committed).toHaveBeenCalledOnce();
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('keeps transport healthy when a paste commit observer throws', async () => {
    const client = createClient('normal');
    await client.start();

    await expect(
      client.injectPaste(undefined, () => {
        throw new Error('application observer failed');
      }),
    ).resolves.toEqual({ submitted: true });
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
    expect(client.readiness.status).toBe('ready');
  });

  it('reports pre-dispatch abort as uncommitted when the helper rejects dispatch', async () => {
    const client = createClient('paste-before-dispatch');
    await client.start();
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = client.injectPaste(controller.signal, committed);
    controller.abort();
    await expect(paste).resolves.toMatchObject({ submitted: false });
    expect(committed).not.toHaveBeenCalled();
  });

  it('closes writes synchronously and never pumps queued work after compromised output', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    controlled.blockNextWrite();
    const first = controlled.client.request('ping', {}, 1_000);
    const second = controlled.client.request('ping', {}, 1_000);
    const firstRejection = first.catch((error: unknown) => error);
    const secondRejection = second.catch((error: unknown) => error);
    const writesBeforeFailure = controlled.writes.length;

    controlled.emitStdout(Buffer.from([0, 0, 0, 1, 0x7b]));

    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    controlled.emitDrain();
    expect(controlled.writes).toHaveLength(writesBeforeFailure);
    expect(await firstRejection).toMatchObject({ code: 'transport-error' });
    expect(await secondRejection).toMatchObject({ code: 'transport-error' });
    expect(controlled.kill).toHaveBeenCalledOnce();
    controlled.close();
    await controlled.client.stop();
  });

  it('removes a pre-dispatch abort from the backpressured queue', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    controlled.blockNextWrite();
    const first = controlled.client.request('ping', {}, 1_000);
    const firstRejection = first.catch((error: unknown) => error);
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = controlled.client.injectPaste(controller.signal, committed);
    const writesBeforeAbort = controlled.writes.length;

    controller.abort();

    await expect(paste).rejects.toMatchObject({ name: 'AbortError' });
    controlled.emitDrain();
    expect(controlled.writes).toHaveLength(writesBeforeAbort);
    expect(committed).not.toHaveBeenCalled();
    controlled.emitStdout(Buffer.from([0, 0, 0, 1, 0x7b]));
    expect(await firstRejection).toMatchObject({ code: 'transport-error' });
    controlled.close();
    await controlled.client.stop();
  });

  it('uses one absolute enqueue-to-response deadline across backpressure', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    vi.useFakeTimers();
    try {
      controlled.blockNextWrite();
      const first = controlled.client.request('ping', {}, 1_000);
      const firstRejection = first.catch((error: unknown) => error);
      const queued = controlled.client.request('ping', {}, 100);
      const queuedRejection = queued.catch((error: unknown) => error);

      await vi.advanceTimersByTimeAsync(90);
      controlled.emitDrain();
      await vi.advanceTimersByTimeAsync(11);

      expect(await queuedRejection).toMatchObject({ code: 'request-timeout' });
      expect(await firstRejection).toMatchObject({ code: 'transport-error' });
      expect(controlled.kill).toHaveBeenCalledOnce();
      controlled.close();
      await controlled.client.stop();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each(['clean stdout EOF', 'truncated stdout', 'child error'] as const)(
    'terminates and closes writes on %s before process close',
    async (failure) => {
      const controlled = createControlledClient();
      await controlled.client.start();
      const writesBeforeFailure = controlled.writes.length;

      if (failure === 'truncated stdout') {
        controlled.emitStdout(Buffer.from([0, 0]));
        controlled.endStdout();
      } else if (failure === 'clean stdout EOF') {
        controlled.endStdout();
      } else {
        controlled.emitChildError();
      }

      await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
      expect(controlled.writes).toHaveLength(writesBeforeFailure);
      expect(controlled.kill).toHaveBeenCalledOnce();
      controlled.close();
      await controlled.client.stop();
    },
  );

  it('restores activation configuration, but not session capture, after restart', async () => {
    let launches = 0;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      spawnHelper: (_path, options) => {
        const scenario = launches++ === 0 ? 'normal' : 'expect-enabled';
        return spawn(process.execPath, [fixture, scenario], {
          ...options,
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      },
    });
    clients.push(client);
    await client.start();
    await client.configureActivation(true, [
      { key: 'Q', shift: false },
      { key: 'Q', shift: true },
    ]);
    await client.setSessionCapture(true);

    await client.restart();
    await waitFor(() => launches >= 2 && client.readiness.status === 'ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('resets uncertain session capture by confirming exit and starting a fresh helper', async () => {
    let launches = 0;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      spawnHelper: (_path, options) => {
        launches += 1;
        return spawn(process.execPath, [fixture, 'normal'], {
          ...options,
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      },
    });
    clients.push(client);
    await client.start();
    await client.setSessionCapture(true);

    await expect(client.resetSessionCapture()).resolves.toBeUndefined();
    expect(launches).toBe(2);
    expect(client.readiness.status).toBe('ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('serializes start during shutdown and isolates throwing observers', async () => {
    const client = createClient('slow-shutdown');
    client.subscribeReadiness(() => {
      throw new Error('observer failure');
    });
    client.subscribeNotifications(() => {
      throw new Error('observer failure');
    });
    await client.start();
    const stopping = client.stop();
    const starting = client.start();
    await Promise.all([stopping, starting]);
    await waitFor(() => client.readiness.status === 'ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('keeps protocol supervision healthy when a notification listener throws', async () => {
    const client = createClient('notify');
    client.subscribeNotifications(() => {
      throw new Error('consumer bug');
    });
    await client.start();
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
    expect(client.readiness.status).toBe('ready');
  });

  it('treats a helper build mismatch as stable incompatible state', async () => {
    const client = createClient('mismatch');
    await client.start();
    await waitFor(() => client.readiness.status === 'incompatible');
    expect(client.readiness).toMatchObject({
      status: 'incompatible',
      reason: 'protocol-mismatch',
    });
    await new Promise((resolveWait) => setTimeout(resolveWait, 350));
    expect(client.readiness.status).toBe('incompatible');
  });

  it('rejects malformed protocol output and opens the crash-loop circuit', async () => {
    const client = createClient('malformed');
    await client.start();
    for (let attempt = 0; attempt < 5; attempt += 1) {
      await waitFor(
        () => client.readiness.status === 'unavailable' && client.readiness.reason !== 'shutdown',
      );
      if (client.readiness.reason === 'crash-loop') break;
      await client.restart();
    }
    await waitFor(() => client.readiness.reason === 'crash-loop');
    expect(client.readiness.status).toBe('unavailable');
  });

  it('surfaces a missing binary without spawning or searching PATH', async () => {
    const client = new HelperClient({
      executablePath: resolve('tmp/tests/does-not-exist/talking-quill-helper'),
      expectedHelperVersion: '1.0.0',
      platform: process.platform === 'win32' ? 'win32' : 'darwin',
      architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
      spawnHelper: () => {
        throw new Error('must not spawn');
      },
    });
    clients.push(client);
    await client.start();
    expect(client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'binary-missing',
    });
  });
});
