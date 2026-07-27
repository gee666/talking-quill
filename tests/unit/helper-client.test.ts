import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HelperClient } from '../../app/src/main/helper/helper-client';
import { encodeHelperFrame, HelperFrameDecoder } from '../../app/src/main/helper/framing';
import type { ActivationBinding } from '../../app/src/shared/helper/protocol';
import {
  shortcutFromLegacyActivation as legacyShortcut,
  type Shortcut,
  type ShortcutKey,
} from '../../app/src/shared/schemas/shortcut';

function shortcutFromLegacyActivation(
  key: ShortcutKey,
  shift: boolean,
  profileId = 'general',
): ActivationBinding {
  return { profileId, shortcut: legacyShortcut(key, shift) };
}

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

function createControlledClient(options: { readonly deferStartupHealth?: boolean } = {}): {
  client: HelperClient;
  writes: Buffer[];
  requests: { readonly id: number; readonly method: string; readonly params: unknown }[];
  launches: () => number;
  blockNextWrite: () => void;
  emitDrain: () => void;
  emitStdout: (chunk: Buffer) => void;
  emitResult: (id: number, result: unknown) => void;
  emitError: (id: number, code?: number) => void;
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
  const requests: { readonly id: number; readonly method: string; readonly params: unknown }[] = [];
  const decoder = new HelperFrameDecoder();
  let blockNext = false;
  let startupHealthResponses = options.deferStartupHealth === true ? 0 : 2;
  const kill = vi.fn(() => true);

  stdin.write = (frame: Buffer): boolean => {
    writes.push(Buffer.from(frame));
    for (const payload of decoder.push(frame)) {
      const request = JSON.parse(payload.toString('utf8')) as {
        id: number;
        method: string;
        params: unknown;
      };
      requests.push(request);
      if (request.method === 'initialize') {
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: {
                protocolVersion: 5,
                helperVersion: '1.0.0',
                platform: process.platform === 'win32' ? 'windows' : 'macos',
                architecture: process.arch === 'arm64' ? 'aarch64' : 'x86_64',
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
      } else if (startupHealthResponses > 0 && request.method === 'permissions.get') {
        startupHealthResponses -= 1;
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: {
                accessibility: 'not_applicable',
                inputMonitoring: 'not_applicable',
                eventPost: 'not_applicable',
              },
            }),
          );
        });
      } else if (startupHealthResponses > 0 && request.method === 'ping') {
        startupHealthResponses -= 1;
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: { ok: true, hookStatus: 'ready' },
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
  let launchCount = 0;
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    spawnHelper: () => {
      launchCount += 1;
      return child;
    },
  });
  clients.push(client);

  return {
    client,
    writes,
    requests,
    launches: () => launchCount,
    blockNextWrite: () => {
      blockNext = true;
    },
    emitDrain: () => stdin.emit('drain'),
    emitStdout: (chunk) => stdout.emit('data', chunk),
    emitResult: (id, result) =>
      stdout.emit('data', encodeHelperFrame({ jsonrpc: '2.0', id, result })),
    emitError: (id, code = -32_003) =>
      stdout.emit(
        'data',
        encodeHelperFrame({
          jsonrpc: '2.0',
          id,
          error: { code, message: 'Native operation unavailable' },
        }),
      ),
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
        shortcutFromLegacyActivation('Z', false),
        shortcutFromLegacyActivation('Z', true, 'prompt'),
      ]),
    ).resolves.toEqual({
      enabled: true,
      bindings: [
        shortcutFromLegacyActivation('Z', false),
        shortcutFromLegacyActivation('Z', true, 'prompt'),
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

  it('deep-clones and freezes retained shortcut intent before replay', async () => {
    const controlled = createControlledClient();
    const input: Shortcut = {
      modifiers: { ctrl: true, alt: false, shift: true, meta: false },
      keys: ['Q', 'P'],
    };
    const configured = await controlled.client.configureActivation(true, [
      { profileId: 'general', shortcut: input },
    ]);

    expect(Object.isFrozen(configured)).toBe(true);
    expect(Object.isFrozen(configured.bindings)).toBe(true);
    expect(Object.isFrozen(configured.bindings[0])).toBe(true);
    expect(Object.isFrozen(configured.bindings[0]?.shortcut)).toBe(true);
    expect(Object.isFrozen(configured.bindings[0]?.shortcut.modifiers)).toBe(true);
    expect(Object.isFrozen(configured.bindings[0]?.shortcut.keys)).toBe(true);

    input.modifiers.ctrl = false;
    input.keys[0] = 'R';
    const starting = controlled.client.start();
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [
          {
            profileId: 'general',
            shortcut: {
              modifiers: { ctrl: true, alt: false, shift: true, meta: false },
              keys: ['Q', 'P'],
            },
          },
        ],
      }),
    );
    const replay = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(replay.id, replay.params);
    await starting;
    controlled.close();
    await controlled.client.stop();
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

  it('processes an adjacent paste commitment before settling its response', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const ordering: string[] = [];
    const paste = controlled.client.injectPaste(undefined, () => ordering.push('committed'));
    const observed = paste.then((result) => {
      ordering.push('settled');
      return result;
    });
    const request = latestRequest(controlled.requests, 'paste.inject');

    controlled.emitStdout(
      Buffer.concat([
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'paste.committed',
          params: { requestId: request.id },
        }),
        encodeHelperFrame({
          jsonrpc: '2.0',
          id: request.id,
          result: { submitted: true },
        }),
      ]),
    );

    await expect(observed).resolves.toEqual({ submitted: true });
    expect(ordering).toEqual(['committed', 'settled']);
    controlled.close();
    await controlled.client.stop();
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
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
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

  it('rejects queued application work when capture reset starts draining', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    controlled.blockNextWrite();
    const firstOutcome = controlled.client
      .request('ping', {}, 1_000)
      .catch((error: unknown) => error);
    const committed = vi.fn();
    const paste = controlled.client.injectPaste(undefined, committed);

    const resetOutcome = controlled.client.resetSessionCapture().catch((error: unknown) => error);
    await expect(paste).rejects.toMatchObject({ code: 'not-running' });
    controlled.emitDrain();

    expect(controlled.requests.some(({ method }) => method === 'paste.inject')).toBe(false);
    expect(committed).not.toHaveBeenCalled();
    const stopping = controlled.client.stop();
    controlled.close();
    await expect(stopping).resolves.toBeUndefined();
    await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
    await expect(firstOutcome).resolves.toMatchObject({ code: 'transport-error' });
  });

  it('does not publish a health snapshot after capture reset starts draining', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const activationCalls = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const health = controlled.client.getPermissions();
    const resetOutcome = controlled.client.resetSessionCapture().catch((error: unknown) => error);

    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'denied',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'unavailable',
    });

    await expect(health).resolves.toMatchObject({ accessibility: 'denied' });
    expect(controlled.client.readiness.status).toBe('ready');
    expect(
      controlled.requests.filter(({ method }) => method === 'activation.configure'),
    ).toHaveLength(activationCalls);
    const stopping = controlled.client.stop();
    controlled.close();
    await expect(stopping).resolves.toBeUndefined();
    await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
  });

  it('does not let pre-drain supervision timeout force-kill graceful reset', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    vi.useFakeTimers();
    try {
      const supervision = controlled.client
        .request('ping', {}, 100, undefined, undefined, 'request-timeout', false, true)
        .catch((error: unknown) => error);
      const resetOutcome = controlled.client.resetSessionCapture().catch((error: unknown) => error);

      await vi.advanceTimersByTimeAsync(101);
      await expect(supervision).resolves.toMatchObject({ code: 'request-timeout' });
      expect(controlled.kill).not.toHaveBeenCalled();

      const stopping = controlled.client.stop();
      controlled.close();
      await expect(stopping).resolves.toBeUndefined();
      await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('bounds all outstanding requests even when writes never backpressure', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baselineWrites = controlled.writes.length;
    const pending = Array.from({ length: 254 }, () =>
      controlled.client.request('ping', {}, 10_000).catch((error: unknown) => error),
    );

    await expect(controlled.client.request('ping', {}, 10_000)).rejects.toMatchObject({
      code: 'request-capacity',
      message: 'Native helper request capacity is full',
    });

    const health = controlled.client.getPermissions();
    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'ready',
    });
    await expect(health).resolves.toMatchObject({ accessibility: 'not_applicable' });
    expect(controlled.writes).toHaveLength(baselineWrites + 256);

    controlled.emitStdout(Buffer.from([0, 0, 0, 1, 0x7b]));
    const errors = await Promise.all(pending);
    expect(errors).toHaveLength(254);
    expect(errors.every((error) => error instanceof Error && 'code' in error)).toBe(true);
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

  it('suspends native activation before publishing permission loss and restores intent before ready', async () => {
    vi.useFakeTimers();
    const controlled = createControlledClient();
    try {
      await controlled.client.start();
      const desired = controlled.client.configureActivation(true, [
        shortcutFromLegacyActivation('Q', false),
      ]);
      await vi.waitFor(() =>
        expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
          enabled: true,
        }),
      );
      controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      });
      await desired;

      const activationCallsBeforeLoss = controlled.requests.filter(
        ({ method }) => method === 'activation.configure',
      ).length;
      await vi.advanceTimersByTimeAsync(5_000);
      const deniedPermissions = latestRequest(controlled.requests, 'permissions.get');
      const unavailablePing = latestRequest(controlled.requests, 'ping');
      controlled.emitResult(deniedPermissions.id, {
        accessibility: 'denied',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      controlled.emitResult(unavailablePing.id, { ok: true, hookStatus: 'unavailable' });
      await vi.waitFor(() =>
        expect(
          controlled.requests.filter(({ method }) => method === 'activation.configure'),
        ).toHaveLength(activationCallsBeforeLoss + 1),
      );
      const suspend = latestRequest(controlled.requests, 'activation.configure');
      expect(suspend.params).toEqual({
        enabled: false,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      });
      expect(controlled.client.readiness.status).toBe('ready');
      controlled.emitResult(suspend.id, {
        enabled: false,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      });
      await vi.waitFor(() =>
        expect(controlled.client.readiness).toMatchObject({
          status: 'permission-required',
          reason: 'accessibility-required',
        }),
      );

      const updatedIntent = controlled.client.configureActivation(true, [
        shortcutFromLegacyActivation('A', true),
      ]);
      await vi.waitFor(() =>
        expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
          enabled: false,
          bindings: [shortcutFromLegacyActivation('A', true)],
        }),
      );
      const suspendedUpdate = latestRequest(controlled.requests, 'activation.configure');
      controlled.emitResult(suspendedUpdate.id, {
        enabled: false,
        bindings: [shortcutFromLegacyActivation('A', true)],
      });
      await updatedIntent;

      await vi.advanceTimersByTimeAsync(5_000);
      const restoredPermissions = latestRequest(controlled.requests, 'permissions.get');
      const readyPing = latestRequest(controlled.requests, 'ping');
      controlled.emitResult(restoredPermissions.id, {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      controlled.emitResult(readyPing.id, { ok: true, hookStatus: 'ready' });
      await vi.waitFor(() =>
        expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
          enabled: true,
        }),
      );
      const restore = latestRequest(controlled.requests, 'activation.configure');
      expect(restore.params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('A', true)],
      });
      expect(controlled.client.readiness.status).toBe('permission-required');
      controlled.emitResult(restore.id, {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('A', true)],
      });
      await vi.waitFor(() => expect(controlled.client.readiness.status).toBe('ready'));
    } finally {
      vi.useRealTimers();
      controlled.close();
      await controlled.client.stop();
    }
  });

  it('rolls concurrent activation failures back to the last committed intent', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const first = controlled.client
      .configureActivation(true, [shortcutFromLegacyActivation('A', false)])
      .catch((error: unknown) => error);
    const second = controlled.client
      .configureActivation(true, [shortcutFromLegacyActivation('B', true)])
      .catch((error: unknown) => error);

    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('A', false)],
      }),
    );
    controlled.emitError(latestRequest(controlled.requests, 'activation.configure').id);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: false,
        bindings: [],
      }),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: false,
      bindings: [],
    });

    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('B', true)],
      }),
    );
    controlled.emitError(latestRequest(controlled.requests, 'activation.configure').id);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: false,
        bindings: [],
      }),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: false,
      bindings: [],
    });

    await expect(first).resolves.toMatchObject({ code: 'rpc-error' });
    await expect(second).resolves.toMatchObject({ code: 'rpc-error' });
    controlled.close();
    await controlled.client.stop();
  });

  it('follows a stale activation enable acknowledgement with disable on newer permission loss', async () => {
    vi.useFakeTimers();
    const controlled = createControlledClient();
    try {
      await controlled.client.start();
      const enabling = controlled.client.configureActivation(true, [
        shortcutFromLegacyActivation('Z', false),
      ]);
      const enablingOutcome = enabling.catch((error: unknown) => error);
      await vi.waitFor(() =>
        expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
          enabled: true,
        }),
      );
      const staleEnable = latestRequest(controlled.requests, 'activation.configure');

      const permissionRequestsBeforeRefresh = controlled.requests.filter(
        ({ method }) => method === 'permissions.get',
      ).length;
      const health = controlled.client.getPermissions();
      const healthOutcome = health.catch((error: unknown) => error);
      await vi.waitFor(() =>
        expect(
          controlled.requests.filter(({ method }) => method === 'permissions.get'),
        ).toHaveLength(permissionRequestsBeforeRefresh + 1),
      );
      controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
        accessibility: 'denied',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
        ok: true,
        hookStatus: 'unavailable',
      });
      for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
      controlled.emitResult(staleEnable.id, {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Z', false)],
      });

      await vi.waitFor(() =>
        expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
          enabled: false,
          bindings: [shortcutFromLegacyActivation('Z', false)],
        }),
      );
      const disable = latestRequest(controlled.requests, 'activation.configure');
      expect(controlled.client.readiness.status).toBe('ready');
      controlled.emitResult(disable.id, {
        enabled: false,
        bindings: [shortcutFromLegacyActivation('Z', false)],
      });

      await expect(enablingOutcome).resolves.toMatchObject({ enabled: true });
      await expect(healthOutcome).resolves.toMatchObject({ accessibility: 'denied' });
      expect(controlled.client.readiness.status).toBe('permission-required');
    } finally {
      vi.useRealTimers();
      controlled.close();
      await controlled.client.stop();
    }
  });

  it('recycles a granted-permission helper whose live hook becomes unavailable', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const desired = controlled.client.configureActivation(true, [
      shortcutFromLegacyActivation('Z', false),
    ]);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
        enabled: true,
      }),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: true,
      bindings: [shortcutFromLegacyActivation('Z', false)],
    });
    await desired;

    const health = controlled.client.getPermissions();
    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'unavailable',
    });
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
        enabled: false,
      }),
    );
    const disable = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(disable.id, {
      enabled: false,
      bindings: [shortcutFromLegacyActivation('Z', false)],
    });

    await health;
    expect(controlled.kill).toHaveBeenCalledOnce();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'hook-fault',
    });
    controlled.close();
    await controlled.client.stop();
  });

  it('bounds restart when a killed child never confirms close', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    vi.useFakeTimers();
    try {
      const restarting = controlled.client.restart();
      const outcome = restarting.catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(1_001);
      await expect(outcome).resolves.toMatchObject({
        code: 'transport-error',
        message: 'Native helper restart could not confirm process exit',
      });
      expect(controlled.client.readiness.status).toBe('unavailable');
    } finally {
      vi.useRealTimers();
      controlled.close();
      await controlled.client.stop();
    }
  });

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
      shortcutFromLegacyActivation('Q', false),
      shortcutFromLegacyActivation('Q', true, 'prompt'),
    ]);
    await client.setSessionCapture(true);

    await client.restart();
    await waitFor(() => launches >= 2 && client.readiness.status === 'ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('confirms startup health before replaying retained enabled activation', async () => {
    const controlled = createControlledClient({ deferStartupHealth: true });
    await controlled.client.configureActivation(true, [shortcutFromLegacyActivation('Q', false)]);

    const starting = controlled.client.start();
    await vi.waitFor(() =>
      expect(controlled.requests.some(({ method }) => method === 'permissions.get')).toBe(true),
    );
    expect(controlled.requests.some(({ method }) => method === 'activation.configure')).toBe(false);
    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'ready',
    });
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      }),
    );
    expect(controlled.client.readiness.status).toBe('starting');
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: true,
      bindings: [shortcutFromLegacyActivation('Q', false)],
    });

    await starting;
    expect(controlled.client.readiness.status).toBe('ready');
    controlled.close();
    await controlled.client.stop();
  });

  it('does not replay retained enabled activation into an unhealthy replacement helper', async () => {
    let launches = 0;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      spawnHelper: (_path, options) => {
        const scenario = launches++ === 0 ? 'normal' : 'permission-required';
        return spawn(process.execPath, [fixture, scenario], {
          ...options,
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      },
    });
    clients.push(client);
    await client.start();
    await client.configureActivation(true, [shortcutFromLegacyActivation('Q', false)]);

    await client.restart();
    await waitFor(() => launches === 2 && client.readiness.status === 'permission-required');

    expect(client.readiness.reason).toBe('accessibility-required');
  });

  it('restarts a permission-created dead hook and restores retained activation on recovery', async () => {
    vi.useFakeTimers();
    let launches = 0;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      spawnHelper: (_path, options) => {
        const scenario = launches++ === 0 ? 'permission-recovers' : 'expect-enabled';
        return spawn(process.execPath, [fixture, scenario], {
          ...options,
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      },
    });
    clients.push(client);
    try {
      await client.configureActivation(true, [
        shortcutFromLegacyActivation('Q', false),
        shortcutFromLegacyActivation('Q', true, 'prompt'),
      ]);
      await client.start();
      expect(client.readiness.status).toBe('permission-required');

      await vi.advanceTimersByTimeAsync(5_000);
      await vi.waitFor(() => expect(client.readiness.status).toBe('unavailable'));
      await vi.advanceTimersByTimeAsync(250);
      await vi.waitFor(() => expect(launches).toBe(2));
      await vi.waitFor(() => expect(client.readiness.status).toBe('ready'));
    } finally {
      vi.useRealTimers();
    }
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

  it('does not let capture reset relaunch after authoritative stop begins', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();

    const reset = controlled.client.resetSessionCapture().catch((error: unknown) => error);
    await vi.waitFor(() =>
      expect(
        controlled.requests.some(
          ({ method, params }) =>
            method === 'session.set_capture' &&
            typeof params === 'object' &&
            params !== null &&
            'active' in params &&
            params.active === false,
        ),
      ).toBe(true),
    );
    const writesBeforeAdmissionCheck = controlled.writes.length;
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    expect(controlled.writes).toHaveLength(writesBeforeAdmissionCheck);

    const stopping = controlled.client.stop();
    controlled.close();

    await expect(stopping).resolves.toBeUndefined();
    await expect(reset).resolves.toMatchObject({ code: 'not-running' });
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
  });

  it('does not let stale reset stop a replacement claimed by newer start intent', async () => {
    let launches = 0;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      spawnHelper: (_path, options) => {
        const scenario = launches++ === 0 ? 'normal' : 'slow-initialize';
        return spawn(process.execPath, [fixture, scenario], {
          ...options,
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      },
    });
    clients.push(client);
    await client.start();

    const reset = client.resetSessionCapture().catch((error: unknown) => error);
    await waitFor(() => launches === 2);
    const newerStart = client.start();

    await expect(newerStart).resolves.toBeUndefined();
    await expect(reset).resolves.toMatchObject({ code: 'not-running' });
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

  it('preserves stopped readiness when stop wins a pending launch validation', async () => {
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

    const starting = client.start();
    await client.stop();
    await starting;
    expect(client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
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

function latestRequest(
  requests: readonly {
    readonly id: number;
    readonly method: string;
    readonly params: unknown;
  }[],
  method: string,
): { readonly id: number; readonly method: string; readonly params: unknown } {
  for (let index = requests.length - 1; index >= 0; index -= 1) {
    const request = requests[index];
    if (request?.method === method) return request;
  }
  throw new Error(`No ${method} request was written`);
}
