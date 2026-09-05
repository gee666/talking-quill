import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  HelperClient,
  activationCaptureRollbackEnabled,
  type HelperClientOptions,
} from '../../app/src/main/helper/helper-client';
import { encodeHelperFrame, HelperFrameDecoder } from '../../app/src/main/helper/framing';
import {
  type ActivationBinding,
  type HelperNotification,
  type HelperRuntimeObservability,
} from '../../app/src/shared/helper/protocol';
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
const ACTIVATION_CONTEXT = Object.freeze({
  activationGeneration: 7,
  targetToken: 'opaque-target',
});
const EXPECTED_CLIPBOARD_SHA256 =
  'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';
const OWNER_SNAPSHOT = Object.freeze({
  model: 'out_of_process',
  protocolVersion: 1,
  state: 'leased_disabled',
  instanceId: 'owner-instance-1',
  buildId: 'owner-build-1',
  leaseEpoch: 1,
  authenticated: true,
} as const);
const clients: HelperClient[] = [];

function unavailableOwnerSnapshot() {
  return {
    ...OWNER_SNAPSHOT,
    state: 'unavailable' as const,
    instanceId: '',
    buildId: '',
    leaseEpoch: null,
    authenticated: false,
  };
}

function initializedResult() {
  return {
    protocolVersion: 10,
    helperVersion: '1.0.0',
    platform: process.platform === 'win32' ? 'windows' : 'macos',
    architecture: process.arch === 'arm64' ? 'aarch64' : 'x86_64',
    hookStatus: 'installed_unobserved',
    permissions: {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    },
    keyboardCapture: {
      activationAvailable: true,
      sessionKeyCaptureAvailable: true,
      runtimeRollbackActive: false,
      buildDisabled: false,
    },
    keyboardOwner: OWNER_SNAPSHOT,
  } as const;
}

afterEach(async () => {
  await Promise.allSettled(clients.splice(0).map((client) => client.stop()));
});

function createClient(
  scenario: string,
  nativeDrainEnvelopeMs = 500,
  observeRuntimeObservability?: (
    observability: HelperRuntimeObservability,
    source: 'runtime' | 'shutdown' | 'failure',
  ) => void,
): HelperClient {
  const platform = process.platform === 'win32' ? 'win32' : 'darwin';
  const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    nativeDrainEnvelopeMs,
    predecessorDrainEnvelopeMs: nativeDrainEnvelopeMs,
    ...(observeRuntimeObservability === undefined ? {} : { observeRuntimeObservability }),
    spawnHelper: (_path, options) =>
      spawn(process.execPath, [fixture, scenario], {
        ...options,
        stdio: ['pipe', 'pipe', 'pipe'],
      }),
  });
  clients.push(client);
  return client;
}

function createCountedProcessClient(scenario: string): {
  readonly client: HelperClient;
  readonly launches: () => number;
  readonly pids: () => readonly number[];
} {
  let launchCount = 0;
  const processIds: number[] = [];
  const platform = process.platform === 'win32' ? 'win32' : 'darwin';
  const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    nativeDrainEnvelopeMs: 100,
    predecessorDrainEnvelopeMs: 100,
    spawnHelper: (_path, options) => {
      launchCount += 1;
      const child = spawn(process.execPath, [fixture, scenario], {
        ...options,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      if (child.pid !== undefined) processIds.push(child.pid);
      return child;
    },
  });
  clients.push(client);
  return { client, launches: () => launchCount, pids: () => [...processIds] };
}

async function waitFor(predicate: () => boolean, timeoutMilliseconds = 4_000): Promise<void> {
  const deadline = Date.now() + timeoutMilliseconds;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('Timed out waiting for helper state');
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
}

function createControlledClient(
  options: {
    readonly deferInitialize?: boolean;
    readonly deferStartupHealth?: boolean;
    readonly deferStartupCapture?: boolean;
    readonly activationCaptureBuildDisabled?: boolean;
    readonly activationCaptureRuntimeRollback?: boolean;
    readonly sessionKeyCaptureAvailable?: boolean;
    readonly initialOwnerUnavailable?: boolean;
    readonly nativeDrainEnvelopeMs?: number;
    readonly predecessorDrainEnvelopeMs?: number;
    readonly replacementChild?: () => ChildProcessWithoutNullStreams;
    readonly platform?: 'win32' | 'darwin';
    readonly observeOwnerConnectionDiagnostic?: HelperClientOptions['observeOwnerConnectionDiagnostic'];
  } = {},
): {
  client: HelperClient;
  writes: Buffer[];
  requests: { readonly id: number; readonly method: string; readonly params: unknown }[];
  launches: () => number;
  blockNextWrite: () => void;
  emitDrain: () => void;
  emitStdout: (chunk: Buffer) => void;
  emitStderr: (chunk: string) => void;
  endStderr: () => void;
  emitResult: (id: number, result: unknown) => void;
  emitError: (id: number, code?: number) => void;
  endStdout: () => void;
  emitChildError: () => void;
  makeStdinUnwritable: () => void;
  completeShutdown: () => void;
  closeSuccessfully: () => void;
  close: () => void;
  closeWithCode: (code: number) => void;
  closeWithSignal: (signal: NodeJS.Signals) => void;
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
  let startupCaptureResponses = options.deferStartupCapture === true ? 0 : 1;
  let startupActivationResponses = options.deferStartupCapture === true ? 0 : 1;
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
      if (request.method === 'initialize' && options.deferInitialize !== true) {
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: {
                protocolVersion: 10,
                helperVersion: '1.0.0',
                platform:
                  (options.platform ?? (process.platform === 'win32' ? 'win32' : 'darwin')) ===
                  'win32'
                    ? 'windows'
                    : 'macos',
                architecture: process.arch === 'arm64' ? 'aarch64' : 'x86_64',
                hookStatus: 'installed_unobserved',
                permissions: {
                  accessibility: 'not_applicable',
                  inputMonitoring: 'not_applicable',
                  eventPost: 'not_applicable',
                },
                keyboardCapture: {
                  activationAvailable:
                    options.initialOwnerUnavailable !== true &&
                    options.activationCaptureBuildDisabled !== true &&
                    options.activationCaptureRuntimeRollback !== true,
                  sessionKeyCaptureAvailable:
                    options.sessionKeyCaptureAvailable ??
                    (options.initialOwnerUnavailable !== true &&
                      options.activationCaptureBuildDisabled !== true &&
                      options.activationCaptureRuntimeRollback !== true),
                  runtimeRollbackActive: options.activationCaptureRuntimeRollback === true,
                  buildDisabled: options.activationCaptureBuildDisabled === true,
                },
                keyboardOwner:
                  options.initialOwnerUnavailable === true
                    ? unavailableOwnerSnapshot()
                    : OWNER_SNAPSHOT,
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
              result: {
                ok: true,
                hookStatus: 'installed_unobserved',
                keyboardOwner:
                  options.initialOwnerUnavailable === true
                    ? unavailableOwnerSnapshot()
                    : OWNER_SNAPSHOT,
              },
            }),
          );
        });
      } else if (
        startupActivationResponses > 0 &&
        request.method === 'activation.configure' &&
        typeof request.params === 'object' &&
        request.params !== null &&
        'enabled' in request.params &&
        request.params.enabled === false
      ) {
        startupActivationResponses -= 1;
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({ jsonrpc: '2.0', id: request.id, result: request.params }),
          );
        });
      } else if (
        startupCaptureResponses > 0 &&
        request.method === 'session.set_capture' &&
        typeof request.params === 'object' &&
        request.params !== null &&
        'mode' in request.params &&
        request.params.mode === 'off'
      ) {
        startupCaptureResponses -= 1;
        queueMicrotask(() => {
          stdout.emit(
            'data',
            encodeHelperFrame({
              jsonrpc: '2.0',
              id: request.id,
              result: { mode: 'off' },
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

  const platform = options.platform ?? (process.platform === 'win32' ? 'win32' : 'darwin');
  const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
  let launchCount = 0;
  const client = new HelperClient({
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform,
    architecture,
    nativeDrainEnvelopeMs: options.nativeDrainEnvelopeMs ?? 1_000,
    predecessorDrainEnvelopeMs:
      options.predecessorDrainEnvelopeMs ?? options.nativeDrainEnvelopeMs ?? 1_000,
    ...(options.observeOwnerConnectionDiagnostic === undefined
      ? {}
      : { observeOwnerConnectionDiagnostic: options.observeOwnerConnectionDiagnostic }),
    spawnHelper: () => {
      launchCount += 1;
      return launchCount === 1 ? child : (options.replacementChild?.() ?? child);
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
    emitStderr: (chunk) => stderr.emit('data', Buffer.from(chunk)),
    endStderr: () => stderr.emit('end'),
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
    makeStdinUnwritable: () => {
      stdin.writable = false;
    },
    completeShutdown: () => {
      const shutdown = latestRequest(requests, 'shutdown');
      stdout.emit(
        'data',
        encodeHelperFrame({
          jsonrpc: '2.0',
          id: shutdown.id,
          result: { ownerDisposition: 'neutral' },
        }),
      );
      stdin.writable = false;
      processEmitter.exitCode = 0;
      processEmitter.emit('close', 0, null);
    },
    closeSuccessfully: () => {
      stdin.writable = false;
      processEmitter.exitCode = 0;
      processEmitter.emit('close', 0, null);
    },
    close: () => {
      stdin.writable = false;
      processEmitter.exitCode = 1;
      processEmitter.emit('close', 1, null);
    },
    closeWithCode: (code) => {
      stdin.writable = false;
      processEmitter.exitCode = code;
      processEmitter.emit('close', code, null);
    },
    closeWithSignal: (signal) => {
      stdin.writable = false;
      processEmitter.signalCode = signal;
      processEmitter.emit('close', null, signal);
    },
    kill,
  };
}

function createAutomaticChild(
  requests: { readonly id: number; readonly method: string; readonly params: unknown }[],
): ChildProcessWithoutNullStreams {
  const stdin = new EventEmitter() as EventEmitter & {
    destroyed: boolean;
    writable: boolean;
    write: (frame: Buffer, callback?: (error?: Error | null) => void) => boolean;
  };
  const stdout = new EventEmitter();
  const stderr = new EventEmitter();
  const decoder = new HelperFrameDecoder();
  stdin.destroyed = false;
  stdin.writable = true;
  let closed = false;

  const processEmitter = new EventEmitter() as EventEmitter & {
    stdin: typeof stdin;
    stdout: EventEmitter;
    stderr: EventEmitter;
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    kill: () => boolean;
  };
  const close = (): void => {
    if (closed) return;
    closed = true;
    stdin.writable = false;
    processEmitter.exitCode = 0;
    processEmitter.emit('close', 0, null);
  };
  const respond = (id: number, result: unknown): void => {
    queueMicrotask(() => {
      stdout.emit('data', encodeHelperFrame({ jsonrpc: '2.0', id, result }));
    });
  };
  stdin.write = (frame): boolean => {
    for (const payload of decoder.push(frame)) {
      const request = JSON.parse(payload.toString('utf8')) as {
        id: number;
        method: string;
        params: unknown;
      };
      requests.push(request);
      if (request.method === 'initialize') {
        respond(request.id, {
          protocolVersion: 10,
          helperVersion: '1.0.0',
          platform: process.platform === 'win32' ? 'windows' : 'macos',
          architecture: process.arch === 'arm64' ? 'aarch64' : 'x86_64',
          hookStatus: 'installed_unobserved',
          permissions: {
            accessibility: 'not_applicable',
            inputMonitoring: 'not_applicable',
            eventPost: 'not_applicable',
          },
          keyboardCapture: {
            activationAvailable: true,
            sessionKeyCaptureAvailable: true,
            runtimeRollbackActive: false,
            buildDisabled: false,
          },
          keyboardOwner: OWNER_SNAPSHOT,
        });
      } else if (request.method === 'permissions.get') {
        respond(request.id, {
          accessibility: 'not_applicable',
          inputMonitoring: 'not_applicable',
          eventPost: 'not_applicable',
        });
      } else if (request.method === 'ping') {
        respond(request.id, {
          ok: true,
          hookStatus: 'installed_unobserved',
          keyboardOwner: OWNER_SNAPSHOT,
        });
      } else if (
        request.method === 'session.set_capture' ||
        request.method === 'activation.configure'
      ) {
        respond(request.id, request.params);
      } else if (request.method === 'shutdown') {
        respond(request.id, { ownerDisposition: 'neutral' });
        queueMicrotask(close);
      }
    }
    return true;
  };
  processEmitter.stdin = stdin;
  processEmitter.stdout = stdout;
  processEmitter.stderr = stderr;
  processEmitter.exitCode = null;
  processEmitter.signalCode = null;
  processEmitter.kill = () => {
    queueMicrotask(close);
    return true;
  };
  return processEmitter as unknown as ChildProcessWithoutNullStreams;
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
    await expect(client.setSessionCapture('recording')).resolves.toEqual({ mode: 'recording' });
    await expect(
      Promise.all([
        client.getFrontApp(),
        client.injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256),
        client.ping(),
      ]),
    ).resolves.toEqual([
      { processName: 'fixture-app', windowTitle: 'Fixture target', windowBounds: null },
      { submitted: true },
      { ok: true, hookStatus: 'installed_unobserved', keyboardOwner: OWNER_SNAPSHOT },
    ]);
    await expect(client.getRuntimeObservability()).resolves.toMatchObject({
      keyboardCapture: {
        runtimeRollbackActive: false,
        activationEnableRequestsBlocked: 0,
        sessionCaptureRequestsBlocked: 0,
        shutdownOwnershipDeadlines: 0,
      },
      transactions: { journalHighWater: 0 },
      replay: { attempted: 0 },
      dummy: { attempted: 0 },
      paste: { attempted: 0, targetValidationFallback: 0 },
    });

    await client.stop();
    expect(client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
  });

  it.each([
    ['owner-state-unavailable', 'owner-missing'],
    ['owner-state-degraded', 'owner-auth-failed'],
    ['owner-state-draining', 'owner-draining'],
    ['owner-state-maintenance', 'owner-maintenance'],
    ['owner-state-idle', 'owner-busy'],
  ] as const)(
    'maps %s to coarse owner readiness without exposing identity',
    async (scenario, reason) => {
      const client = createClient(scenario);
      await client.start();
      expect(client.readiness).toMatchObject({ status: 'unavailable', reason });
      expect(client.readiness).not.toHaveProperty('keyboardOwner');
    },
  );

  it('reconciles an owner that authenticates after an unavailable boot snapshot', async () => {
    const controlled = createControlledClient({ initialOwnerUnavailable: true });
    await controlled.client.start();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'owner-missing',
    });

    const captureBaseline = controlled.requests.filter(
      ({ method }) => method === 'session.set_capture',
    ).length;
    const activationBaseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const health = controlled.client.getPermissions();
    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'session.set_capture'),
      ).toHaveLength(captureBaseline + 1),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'session.set_capture').id, {
      mode: 'off',
    });
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(activationBaseline + 1),
    );
    const activation = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(activation.id, activation.params);

    await expect(health).resolves.toMatchObject({ accessibility: 'not_applicable' });
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness.status).toBe('ready');
    expect(controlled.client.sessionKeyCaptureAvailable).toBe(true);
    controlled.close();
    await controlled.client.stop();
  });

  it('recycles a gateway after two missing-owner health checks', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const controlled = createControlledClient();
    try {
      await controlled.client.start();
      for (let check = 0; check < 2; check += 1) {
        await vi.advanceTimersByTimeAsync(5_000);
        controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
          accessibility: 'not_applicable',
          inputMonitoring: 'not_applicable',
          eventPost: 'not_applicable',
        });
        controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
          ok: true,
          hookStatus: 'unavailable',
          keyboardOwner: unavailableOwnerSnapshot(),
        });
        await Promise.resolve();
        await Promise.resolve();
      }
      await vi.waitFor(() =>
        expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(true),
      );
      controlled.completeShutdown();
      controlled.closeSuccessfully();
      await controlled.client.stop();
    } finally {
      controlled.close();
      await controlled.client.stop();
      vi.useRealTimers();
    }
  });

  it.each(['owner-instance-mismatch', 'owner-epoch-mismatch'] as const)(
    'rejects a ping %s before owner state can become authoritative',
    async (scenario) => {
      const client = createClient(scenario);
      await client.start();
      expect(client.readiness).toMatchObject({ status: 'unavailable', reason: 'owner-degraded' });
      expect(client.activationCaptureEnabled).toBeNull();
    },
  );

  it('replays only the latest authoritative gesture across health and owner reconciliation', async () => {
    const controlled = createControlledClient();
    const notifications: HelperNotification[] = [];
    controlled.client.subscribeNotifications((notification) => notifications.push(notification));
    await controlled.client.start();
    const shortcut = shortcutFromLegacyActivation('Z', false);
    const desired = controlled.client.configureActivation(true, [shortcut]);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
        enabled: true,
      }),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: true,
      bindings: [shortcut],
    });
    await desired;
    const captureBaseline = controlled.requests.filter(
      ({ method }) => method === 'session.set_capture',
    ).length;
    const activationBaseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const health = controlled.client.getPermissions();
    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });
    controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: { ...OWNER_SNAPSHOT, instanceId: 'local-owner-2', leaseEpoch: 2 },
    });
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'session.set_capture'),
      ).toHaveLength(captureBaseline + 1),
    );
    const captureOff = latestRequest(controlled.requests, 'session.set_capture');
    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          ...shortcut,
          activationGeneration: 1,
          targetToken: 'superseded-target',
        },
      }),
    );
    controlled.emitResult(captureOff.id, { mode: 'off' });
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(activationBaseline + 1),
    );
    expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
      enabled: false,
      bindings: [shortcut],
    });
    const disabled = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitStdout(
      Buffer.concat([
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            phase: 'down',
            ...shortcut,
            activationGeneration: 2,
            targetToken: 'current-target',
          },
        }),
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            phase: 'up',
            ...shortcut,
            activationGeneration: 2,
            targetToken: 'current-target',
          },
        }),
      ]),
    );
    controlled.emitResult(disabled.id, disabled.params);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcut],
      }),
    );
    const enabled = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(enabled.id, enabled.params);

    await expect(health).resolves.toMatchObject({ accessibility: 'not_applicable' });
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness.status).toBe('ready');
    expect(
      notifications.map((notification) =>
        notification.method === 'activation.event'
          ? [notification.params.phase, notification.params.activationGeneration]
          : null,
      ),
    ).toEqual([
      ['down', 2],
      ['up', 2],
    ]);
    expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    controlled.close();
    await controlled.client.stop();
  });

  it('keeps a healthy gateway through transient owner drain and recovers on heartbeat', async () => {
    vi.useFakeTimers();
    const controlled = createControlledClient();
    try {
      await controlled.client.start();
      const permissionsBaseline = controlled.requests.filter(
        ({ method }) => method === 'permissions.get',
      ).length;
      const pingBaseline = controlled.requests.filter(({ method }) => method === 'ping').length;
      const activationBaseline = controlled.requests.filter(
        ({ method }) => method === 'activation.configure',
      ).length;
      const health = controlled.client.getPermissions();
      controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
        ok: true,
        hookStatus: 'installed_unobserved',
        keyboardOwner: { ...OWNER_SNAPSHOT, state: 'draining' },
      });
      await health;

      expect(controlled.client.readiness).toMatchObject({
        status: 'unavailable',
        reason: 'owner-draining',
      });
      expect(controlled.launches()).toBe(1);
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);

      await vi.advanceTimersByTimeAsync(5_000);
      expect(controlled.requests.filter(({ method }) => method === 'permissions.get')).toHaveLength(
        permissionsBaseline + 2,
      );
      controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      await vi.waitFor(() =>
        expect(controlled.requests.filter(({ method }) => method === 'ping')).toHaveLength(
          pingBaseline + 2,
        ),
      );
      controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
        ok: true,
        hookStatus: 'installed_unobserved',
        keyboardOwner: OWNER_SNAPSHOT,
      });
      await vi.waitFor(() =>
        expect(
          controlled.requests.filter(({ method }) => method === 'activation.configure'),
        ).toHaveLength(activationBaseline + 1),
      );
      expect(latestRequest(controlled.requests, 'activation.configure').params).toMatchObject({
        enabled: false,
      });
      const activation = latestRequest(controlled.requests, 'activation.configure');
      controlled.emitResult(activation.id, activation.params);
      await vi.waitFor(() => expect(controlled.client.readiness.status).toBe('ready'));

      expect(controlled.launches()).toBe(1);
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    } finally {
      controlled.close();
      await controlled.client.stop();
      vi.useRealTimers();
    }
  });

  it('fences a same-chunk activation behind ordinary ping owner reconciliation', async () => {
    const controlled = createControlledClient();
    const notifications: HelperNotification[] = [];
    controlled.client.subscribeNotifications((notification) => notifications.push(notification));
    await controlled.client.start();
    const captureBaseline = controlled.requests.filter(
      ({ method }) => method === 'session.set_capture',
    ).length;
    const activationBaseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const ping = controlled.client.ping();
    const pingRequest = latestRequest(controlled.requests, 'ping');
    const shortcut = shortcutFromLegacyActivation('Z', false);
    controlled.emitStdout(
      Buffer.concat([
        encodeHelperFrame({
          jsonrpc: '2.0',
          id: pingRequest.id,
          result: {
            ok: true,
            hookStatus: 'installed_unobserved',
            keyboardOwner: { ...OWNER_SNAPSHOT, instanceId: 'local-owner-3', leaseEpoch: 3 },
          },
        }),
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            phase: 'complete',
            ...shortcut,
            activationGeneration: 1,
            targetToken: null,
            heldMs: 10,
          },
        }),
      ]),
    );
    await expect(ping).resolves.toMatchObject({ ok: true });
    expect(notifications).toHaveLength(0);

    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'session.set_capture'),
      ).toHaveLength(captureBaseline + 1),
    );
    const captureOff = latestRequest(controlled.requests, 'session.set_capture');
    controlled.emitResult(captureOff.id, { mode: 'off' });
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(activationBaseline + 1),
    );
    const disabled = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(disabled.id, disabled.params);

    await vi.waitFor(() => expect(notifications).toHaveLength(1));
    expect(controlled.client.readiness.status).toBe('ready');
    expect(controlled.launches()).toBe(1);
    expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    controlled.close();
    await controlled.client.stop();
  });

  it.each([
    ['owner-rpc-auth', 'owner-auth-failed'],
    ['owner-rpc-incompatible', 'owner-incompatible'],
    ['owner-rpc-busy', 'owner-busy'],
    ['owner-rpc-draining', 'owner-draining'],
    ['owner-rpc-rollback', 'owner-rollback'],
    ['owner-rpc-security', 'owner-security-fault'],
    ['owner-rpc-indeterminate', 'owner-indeterminate'],
  ] as const)('preserves exact %s startup error classification', async (scenario, reason) => {
    const client = createClient(scenario);
    await client.start();
    expect(client.readiness.reason).toBe(reason);
  });

  it.each([
    [
      'an incompatible owner',
      'talking-quill-helper: keyboard owner is incompatible\n',
      'owner-incompatible',
      'owner-incompatible',
      'incompatible',
    ],
  ] as const)(
    'waits for stderr after stdout closes and preserves %s',
    async (_case, stderr, nativeFailure, reason, status) => {
      const controlled = createControlledClient({ deferInitialize: true });
      const starting = controlled.client.start();
      await waitFor(() => controlled.requests.some(({ method }) => method === 'initialize'));

      controlled.endStdout();
      await new Promise((resolveWait) => setTimeout(resolveWait, 10));
      expect(controlled.client.readiness.reason).not.toBe(reason);
      controlled.emitStderr(stderr);
      controlled.endStderr();
      controlled.close();

      await starting;
      await new Promise((resolveWait) => setTimeout(resolveWait, 400));
      expect(controlled.launches()).toBe(1);
      expect(controlled.client.nativeLaunchFailure).toBe(nativeFailure);
      expect(controlled.client.readiness).toMatchObject({ status, reason });
    },
  );

  it.each([
    [
      'an exit code',
      (controlled: ReturnType<typeof createControlledClient>) => controlled.close(),
      'helper-exit-code-1',
    ],
    [
      'a signal',
      (controlled: ReturnType<typeof createControlledClient>) =>
        controlled.closeWithSignal('SIGTERM'),
      'helper-exit-signal-sigterm',
    ],
  ] as const)(
    'retains a bounded child diagnostic for %s without stderr',
    async (_case, close, expected) => {
      const controlled = createControlledClient({ deferInitialize: true });
      const starting = controlled.client.start();
      await waitFor(() => controlled.requests.some(({ method }) => method === 'initialize'));
      close(controlled);
      await starting;
      expect(controlled.client.nativeLaunchFailure).toBe(expected);
    },
  );

  it.each([
    [-32_005, 'owner-auth-failed'],
    [-32_009, 'owner-rollback'],
    [-32_010, 'owner-security-fault'],
    [-32_011, 'owner-indeterminate'],
    [-32_012, 'owner-singleton-collision'],
  ] as const)('does not churn protected roles after stable owner RPC %s', async (code, reason) => {
    const controlled = createControlledClient({ deferInitialize: true });
    const starting = controlled.client.start();
    await waitFor(() => controlled.requests.some(({ method }) => method === 'initialize'));
    const initialize = latestRequest(controlled.requests, 'initialize');
    controlled.emitError(initialize.id, code);
    await starting;
    await new Promise((resolveWait) => setTimeout(resolveWait, 400));
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness).toMatchObject({ status: 'unavailable', reason });
  });

  it('requires an exact neutral owner disposition when readiness shutdown requests it', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const stopping = controlled.client.stop({ requireNeutral: true });
    await waitFor(() => controlled.requests.some(({ method }) => method === 'shutdown'));
    const shutdown = latestRequest(controlled.requests, 'shutdown');
    controlled.emitResult(shutdown.id, { ownerDisposition: 'draining' });
    controlled.closeSuccessfully();
    await expect(stopping).rejects.toMatchObject({ code: 'transport-error' });
    expect(controlled.launches()).toBe(1);
  });

  it('sends strict maintenance metadata once without using the ordinary timeout clamp', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const params = {
      operation: 'update',
      transactionId: 'transaction-1',
      sourceBuildId: 'source-build',
      targetBuildId: 'target-build',
      targetOwnerSha256: 'a'.repeat(64),
    } as const;
    const maintenance = controlled.client.prepareOwnerMaintenance(params, 10_000);
    const request = latestRequest(controlled.requests, 'owner.prepare_maintenance');
    expect(request.params).toEqual(params);
    await expect(controlled.client.prepareOwnerMaintenance(params, 10_000)).rejects.toMatchObject({
      code: 'not-running',
    });
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    await expect(
      controlled.client.configureActivation(true, [shortcutFromLegacyActivation('Q', false)]),
    ).rejects.toMatchObject({ code: 'not-running' });
    expect(
      controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
    ).toHaveLength(1);
    controlled.emitResult(request.id, {
      maintenanceReady: true,
      ownerHandoff: '11'.repeat(32),
    });
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await expect(maintenance).resolves.toEqual({
      maintenanceReady: true,
      ownerHandoff: '11'.repeat(32),
    });
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'owner-maintenance',
    });
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    expect(
      controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
    ).toHaveLength(1);
  });

  it('launches the production helper without runtime-selection arguments', () => {
    const source = readFileSync('app/src/main/helper/helper-process.ts', 'utf8');
    expect(source).toContain('return spawn(executablePath, [], {');
    expect(source).not.toContain('diagnosticCapabilityId');
    expect(source).not.toContain('launchReadinessCorrelation');
  });

  it('does not dispatch retained activation before startup disabled-first reconciliation', async () => {
    const controlled = createControlledClient({ deferInitialize: true });
    const starting = controlled.client.start();
    await vi.waitFor(() =>
      expect(controlled.requests.map(({ method }) => method)).toEqual(['initialize']),
    );
    const desired = controlled.client.configureActivation(true, [
      shortcutFromLegacyActivation('Q', false),
    ]);
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    await expect(
      controlled.client.prepareOwnerMaintenance(
        {
          operation: 'uninstall',
          transactionId: 'transaction-1',
          sourceBuildId: 'source-build',
        },
        1_000,
      ),
    ).rejects.toMatchObject({ code: 'not-running' });
    await Promise.resolve();
    expect(controlled.requests.map(({ method }) => method)).toEqual(['initialize']);

    controlled.emitResult(latestRequest(controlled.requests, 'initialize').id, initializedResult());
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      }),
    );
    expect(controlled.requests.map(({ method }) => method)).toEqual([
      'initialize',
      'permissions.get',
      'ping',
      'session.set_capture',
      'activation.configure',
      'activation.configure',
    ]);
    expect(controlled.requests.at(-2)?.params).toEqual({
      enabled: false,
      bindings: [shortcutFromLegacyActivation('Q', false)],
    });
    const enable = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(enable.id, enable.params);
    await expect(starting).resolves.toBeUndefined();
    await expect(desired).resolves.toMatchObject({ enabled: true });
  });

  it('restores supervision after maintenance is aborted before dispatch', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const abort = new AbortController();
    abort.abort();
    await expect(
      controlled.client.prepareOwnerMaintenance(
        {
          operation: 'uninstall',
          transactionId: 'transaction-1',
          sourceBuildId: 'source-build',
        },
        1_000,
        abort.signal,
      ),
    ).rejects.toMatchObject({ name: 'AbortError' });
    expect(
      controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
    ).toHaveLength(0);
    const ping = controlled.client.ping();
    const request = latestRequest(controlled.requests, 'ping');
    controlled.emitResult(request.id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await expect(ping).resolves.toMatchObject({ ok: true });
    expect(controlled.client.readiness.status).toBe('ready');
  });

  it.each([0, -1, Number.NaN, Number.POSITIVE_INFINITY])(
    'rejects invalid maintenance deadline %s before dispatch',
    async (timeoutMs) => {
      const controlled = createControlledClient();
      await controlled.client.start();
      await expect(
        controlled.client.prepareOwnerMaintenance(
          {
            operation: 'uninstall',
            transactionId: 'transaction-1',
            sourceBuildId: 'source-build',
          },
          timeoutMs,
        ),
      ).rejects.toMatchObject({ code: 'request-timeout' });
      expect(
        controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
      ).toHaveLength(0);
      expect(controlled.client.readiness.status).toBe('ready');
    },
  );

  it('applies an absolute pre-dispatch maintenance deadline without writing the mutation', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const predecessor = controlled.client.ping().catch((error: unknown) => error);
    const params = {
      operation: 'uninstall',
      transactionId: 'transaction-1',
      sourceBuildId: 'source-build',
    } as const;
    const maintenance = controlled.client
      .prepareOwnerMaintenance(params, 20)
      .catch((error: unknown) => error);
    await expect(maintenance).resolves.toMatchObject({ code: 'request-timeout' });
    expect(
      controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
    ).toHaveLength(0);
    controlled.close();
    await expect(predecessor).resolves.toBeInstanceOf(Error);
  });

  it('does not retry an uncertain dispatched maintenance mutation', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const maintenance = controlled.client
      .prepareOwnerMaintenance(
        {
          operation: 'uninstall',
          transactionId: 'transaction-1',
          sourceBuildId: 'source-build',
        },
        20,
      )
      .catch((error: unknown) => error);
    await expect(maintenance).resolves.toMatchObject({ code: 'request-timeout' });
    expect(
      controlled.requests.filter(({ method }) => method === 'owner.prepare_maintenance'),
    ).toHaveLength(1);
  });

  it('includes post-ack gateway exit in the absolute maintenance deadline', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 1_000 });
    await controlled.client.start();
    const startedAt = Date.now();
    const maintenance = controlled.client
      .prepareOwnerMaintenance(
        {
          operation: 'uninstall',
          transactionId: 'transaction-1',
          sourceBuildId: 'source-build',
        },
        100,
      )
      .catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'owner.prepare_maintenance');
    await new Promise((resolveWait) => setTimeout(resolveWait, 70));
    controlled.emitResult(request.id, {
      maintenanceReady: true,
      ownerHandoff: '11'.repeat(32),
    });
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    await expect(maintenance).resolves.toMatchObject({ code: 'request-timeout' });
    expect(Date.now() - startedAt).toBeLessThan(250);
    controlled.closeSuccessfully();
  });

  it('accepts owner draining as a clean gateway shutdown disposition', async () => {
    const client = createClient('shutdown-draining');
    await client.start();
    await expect(client.stop()).resolves.toBeUndefined();
    expect(client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
  });

  it('accepts only strict aggregate terminal stderr after helper quiescence', async () => {
    const observations: {
      readonly observability: HelperRuntimeObservability;
      readonly source: 'runtime' | 'shutdown' | 'failure';
    }[] = [];
    const client = createClient('terminal-observability', 500, (observability, source) => {
      observations.push({ observability, source });
    });
    await client.start();

    await client.getRuntimeObservability();
    expect(observations).toHaveLength(1);
    await client.stop();

    expect(observations).toHaveLength(2);
    expect(observations[0]).toMatchObject({
      source: 'runtime',
      observability: { transactions: { cancelled: 0 } },
    });
    expect(observations[1]).toMatchObject({
      source: 'shutdown',
      observability: {
        transactions: { cancelled: 1, cancellationReasons: { shutdown: 1 } },
        paste: { nativeWaitDurationMsTotal: 75, nativeWaitDurationMsMax: 75 },
      },
    });
    expect(JSON.stringify(observations)).not.toContain('must-not-cross-sink');
  });

  it('durably commits strict privacy-safe replay before sending a framed ACK', async () => {
    const diagnostics: unknown[] = [];
    const controlled = createControlledClient({
      observeOwnerConnectionDiagnostic: (diagnostic) => {
        diagnostics.push(diagnostic);
        return Promise.resolve(true);
      },
    });
    await controlled.client.start();
    const valid = {
      event: 'helper.owner.connection.replay',
      journalId: '01'.repeat(32),
      journalNonce: '23'.repeat(32),
      streamId: '45'.repeat(32),
      processGeneration: '1',
      category: 'disconnected',
      operation: 'lease.renew',
      correlationStatus: 'pending',
      healthRefresh: 'not_attempted',
      transportStatus: 'eof',
      ownerProcessState: 'running',
      count: '37',
      counterOverflow: false,
      durable: true,
      durabilityFailures: '0',
      writerStartFailures: '0',
      synchronizationRecoveries: '0',
    } as const;
    controlled.emitStderr(`${JSON.stringify(valid)}\n`);
    controlled.emitStderr(`${JSON.stringify({ ...valid, token: 'must-not-cross-sink' })}\n`);
    await vi.waitFor(() => expect(diagnostics).toHaveLength(1));
    expect(diagnostics[0]).toEqual(valid);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'diagnostic.ack')).toBeDefined(),
    );
    const ack = latestRequest(controlled.requests, 'diagnostic.ack');
    expect(ack.params).toEqual({
      journalId: valid.journalId,
      journalNonce: valid.journalNonce,
      dimensions: {
        category: valid.category,
        operation: valid.operation,
        correlationStatus: valid.correlationStatus,
        healthRefresh: valid.healthRefresh,
        transportStatus: valid.transportStatus,
        ownerProcessState: valid.ownerProcessState,
      },
      count: '37',
    });
    controlled.emitResult(ack.id, { acknowledged: true });
    const stop = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await stop;
  });

  it('rejects sentinel and malformed replay identities before observation or ACK', async () => {
    const diagnostics: unknown[] = [];
    const controlled = createControlledClient({
      observeOwnerConnectionDiagnostic: (diagnostic) => {
        diagnostics.push(diagnostic);
        return Promise.resolve(true);
      },
    });
    await controlled.client.start();
    const valid = {
      event: 'helper.owner.connection.replay',
      journalId: '01'.repeat(32),
      journalNonce: '23'.repeat(32),
      streamId: '45'.repeat(32),
      processGeneration: '1',
      category: 'disconnected',
      operation: 'lease.renew',
      correlationStatus: 'pending',
      healthRefresh: 'not_attempted',
      transportStatus: 'eof',
      ownerProcessState: 'running',
      count: '1',
      counterOverflow: false,
      durable: true,
      durabilityFailures: '0',
      writerStartFailures: '0',
      synchronizationRecoveries: '0',
    } as const;
    for (const sentinel of ['0'.repeat(64), 'f'.repeat(64)]) {
      for (const field of ['journalId', 'journalNonce', 'streamId'] as const) {
        controlled.emitStderr(`${JSON.stringify({ ...valid, [field]: sentinel })}\n`);
      }
    }
    controlled.emitStderr(`${JSON.stringify({ ...valid, streamId: '45'.repeat(31) })}\n`);
    await new Promise((resolveWait) => setTimeout(resolveWait, 25));
    expect(diagnostics).toEqual([]);
    expect(controlled.requests.some(({ method }) => method === 'diagnostic.ack')).toBe(false);

    controlled.emitStderr(`${JSON.stringify(valid)}\n`);
    await vi.waitFor(() => expect(diagnostics).toEqual([valid]));
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'diagnostic.ack')).toBeDefined(),
    );
    const ack = latestRequest(controlled.requests, 'diagnostic.ack');
    controlled.emitResult(ack.id, { acknowledged: true });
    const stop = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await stop;
  });

  it('does not ACK before commit and retries safely after an ACK response is lost', async () => {
    const releaseCommits: ((value: boolean) => void)[] = [];
    let commits = 0;
    const controlled = createControlledClient({
      observeOwnerConnectionDiagnostic: () => {
        commits += 1;
        return new Promise<boolean>((resolveCommit) => {
          releaseCommits.push(resolveCommit);
        });
      },
    });
    await controlled.client.start();
    const replay = {
      event: 'helper.owner.connection.replay',
      journalId: '01'.repeat(32),
      journalNonce: '23'.repeat(32),
      streamId: '45'.repeat(32),
      processGeneration: '1',
      category: 'disconnected',
      operation: 'lease.renew',
      correlationStatus: 'pending',
      healthRefresh: 'not_attempted',
      transportStatus: 'eof',
      ownerProcessState: 'running',
      count: '40',
      counterOverflow: false,
      durable: true,
      durabilityFailures: '0',
      writerStartFailures: '0',
      synchronizationRecoveries: '0',
    } as const;
    controlled.emitStderr(`${JSON.stringify(replay)}\n`);
    await vi.waitFor(() => expect(commits).toBe(1));
    expect(controlled.requests.some(({ method }) => method === 'diagnostic.ack')).toBe(false);
    releaseCommits.shift()?.(true);
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'diagnostic.ack')).toBeDefined(),
    );
    const firstAck = latestRequest(controlled.requests, 'diagnostic.ack');
    controlled.emitStderr(`${JSON.stringify(replay)}\n`);
    await new Promise((resolveWait) => setTimeout(resolveWait, 10));
    expect(commits).toBe(1);
    controlled.emitError(firstAck.id);
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
    controlled.emitStderr(`${JSON.stringify(replay)}\n`);
    await vi.waitFor(() => expect(commits).toBe(2));
    releaseCommits.shift()?.(true);
    await vi.waitFor(() =>
      expect(controlled.requests.filter(({ method }) => method === 'diagnostic.ack')).toHaveLength(
        2,
      ),
    );
    const secondAck = latestRequest(controlled.requests, 'diagnostic.ack');
    controlled.emitResult(secondAck.id, { acknowledged: true });
    const stop = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await stop;
  });

  it('rejects duplicate terminal aggregate candidates from one helper child', async () => {
    const observations: HelperRuntimeObservability[] = [];
    const client = createClient('terminal-observability-duplicate', 500, (observability) => {
      observations.push(observability);
    });
    await client.start();
    await client.stop();
    expect(observations).toEqual([]);
  });

  it('retains clean helper terminal snapshots across host-driven restart', async () => {
    const sources: string[] = [];
    const client = createClient('terminal-observability', 500, (_observability, source) => {
      sources.push(source);
    });
    await client.start();
    await client.restart();
    expect(sources).toEqual(['shutdown']);
    await client.stop();
    expect(sources).toEqual(['shutdown', 'shutdown']);
  });

  it('rejects a terminal outcome that disagrees with the confirmed child exit', async () => {
    const observations: HelperRuntimeObservability[] = [];
    const client = createClient('terminal-observability-mismatched', 500, (observability) => {
      observations.push(observability);
    });
    await client.start();
    await client.stop();
    expect(observations).toEqual([]);
  });

  it('enables runtime rollback only for the exact internal environment marker', () => {
    expect(activationCaptureRollbackEnabled({})).toBe(false);
    expect(
      activationCaptureRollbackEnabled({ TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE: '0' }),
    ).toBe(false);
    expect(
      activationCaptureRollbackEnabled({ TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE: 'true' }),
    ).toBe(false);
    expect(
      activationCaptureRollbackEnabled({ TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE: '1' }),
    ).toBe(true);
  });

  it('treats a contradictory all-or-nothing keyboard capture handshake as malformed', async () => {
    const controlled = createControlledClient({ sessionKeyCaptureAvailable: false });
    await controlled.client.start();
    await vi.waitFor(() =>
      expect(controlled.client.readiness).toMatchObject({
        status: 'unavailable',
        reason: 'malformed-response',
      }),
    );
    expect(controlled.client.sessionKeyCaptureAvailable).toBeNull();
    controlled.close();
    await controlled.client.stop();
  });

  it.each([
    ['build disable', { activationCaptureBuildDisabled: true }, 'capture-disabled'],
    ['runtime rollback', { activationCaptureRuntimeRollback: true }, 'owner-rollback'],
  ] as const)(
    'preserves owner-disabled readiness through heartbeat for %s',
    async (_label, options, expectedReason) => {
      const controlled = createControlledClient(options);
      const binding = shortcutFromLegacyActivation('Q', false);
      await controlled.client.configureActivation(true, [binding]);
      await controlled.client.start();
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: false,
        bindings: [binding],
      });
      expect(controlled.client.readiness).toMatchObject({
        status: 'unavailable',
        reason: expectedReason,
        helperVersion: '1.0.0',
      });
      expect(controlled.client.activationCaptureEnabled).toBe(false);
      expect(controlled.client.sessionKeyCaptureAvailable).toBe(false);
      await expect(controlled.client.setSessionCapture('recording')).resolves.toEqual({
        mode: 'off',
      });
      expect(latestRequest(controlled.requests, 'session.set_capture').params).toEqual({
        mode: 'off',
      });

      const refresh = controlled.client.getPermissions();
      await vi.waitFor(() =>
        expect(
          controlled.requests.filter(({ method }) => method === 'permissions.get'),
        ).toHaveLength(2),
      );
      controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      await vi.waitFor(() =>
        expect(controlled.requests.filter(({ method }) => method === 'ping')).toHaveLength(2),
      );
      controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
        ok: true,
        hookStatus: 'unavailable',
        keyboardOwner: OWNER_SNAPSHOT,
      });
      await expect(refresh).resolves.toMatchObject({ accessibility: 'not_applicable' });
      expect(
        controlled.requests
          .filter(({ method }) => method === 'activation.configure')
          .map(({ params }) => params),
      ).toEqual([{ enabled: false, bindings: [binding] }]);
      expect(controlled.client.readiness).toMatchObject({
        status: 'unavailable',
        reason: expectedReason,
      });

      controlled.closeSuccessfully();
      await expect(controlled.client.stop()).resolves.toBeUndefined();
    },
  );

  it('preserves the Windows launch environment and controls the rollback marker', async () => {
    const requests: { readonly id: number; readonly method: string; readonly params: unknown }[] =
      [];
    let helperEnvironment: NodeJS.ProcessEnv | undefined;
    const platform = process.platform === 'win32' ? 'win32' : 'darwin';
    const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform,
      architecture,
      disableActivationCapture: true,
      nativeDrainEnvelopeMs: 500,
      spawnHelper: (_path, options) => {
        helperEnvironment = options.env;
        return createAutomaticChild(requests);
      },
    });
    clients.push(client);

    await client.start();
    expect(helperEnvironment).toMatchObject({
      ...process.env,
      NO_COLOR: '1',
      TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE: '1',
    });
  });

  it.runIf(process.platform === 'win32')(
    'keeps passive observation disabled across health checks until explicitly stopped',
    async () => {
      const requests: { readonly id: number; readonly method: string; readonly params: unknown }[] =
        [];
      const client = new HelperClient({
        executablePath: process.execPath,
        expectedHelperVersion: '1.0.0',
        platform: 'win32',
        architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
        nativeDrainEnvelopeMs: 500,
        spawnHelper: () => createAutomaticChild(requests),
      });
      clients.push(client);
      await client.start();
      await client.configureActivation(true, [shortcutFromLegacyActivation('X', false)]);
      vi.spyOn(client, 'getRuntimeObservability').mockResolvedValue(
        {} as HelperRuntimeObservability,
      );
      await client.beginPhysicalObservation();
      const baseline = requests.length;
      await client.getPermissions();
      await client.getPermissions();
      expect(client.activationCaptureEnabled).toBe(false);
      expect(
        requests.slice(baseline).filter(({ method }) => method === 'activation.configure'),
      ).toEqual([]);
      await client.endPhysicalObservation();
      expect(client.activationCaptureEnabled).toBe(true);
    },
  );

  it.runIf(process.platform === 'win32')(
    'confirms neutral passive configuration before taking the observation baseline',
    async () => {
      const requests: { readonly id: number; readonly method: string; readonly params: unknown }[] =
        [];
      const client = new HelperClient({
        executablePath: process.execPath,
        expectedHelperVersion: '1.0.0',
        platform: 'win32',
        architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
        nativeDrainEnvelopeMs: 500,
        spawnHelper: () => createAutomaticChild(requests),
      });
      clients.push(client);
      await client.start();
      const requestBaseline = requests.length;
      const expectedBaseline = {} as HelperRuntimeObservability;
      const readBaseline = vi.spyOn(client, 'getRuntimeObservability').mockImplementation(() => {
        expect(requests.slice(requestBaseline).map(({ method }) => method)).toEqual([
          'activation.configure',
        ]);
        expect(requests.at(-1)?.params).toEqual({ enabled: false, bindings: [] });
        return Promise.resolve(expectedBaseline);
      });

      await expect(client.beginPhysicalObservation()).resolves.toBe(expectedBaseline);
      expect(readBaseline).toHaveBeenCalledOnce();
    },
  );

  it.each([
    [
      'changed bindings',
      true,
      {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('R', false)],
      },
    ],
    [
      'an enable upgrade',
      false,
      {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      },
    ],
    [
      'a disable downgrade',
      true,
      {
        enabled: false,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      },
    ],
  ] as const)('rejects activation acknowledgements with %s', async (_name, enabled, response) => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const binding = shortcutFromLegacyActivation('Q', false);

    const configuring = controlled.client.configureActivation(enabled, [binding]);
    const outcome = configuring.catch((error: unknown) => error);
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(2),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, response);

    await expect(outcome).resolves.toMatchObject({
      code: 'rpc-error',
      message: 'Native helper returned a mismatched activation configuration',
    });
    await vi.waitFor(() =>
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(true),
    );
    controlled.completeShutdown();
    await Promise.resolve();
    expect(controlled.kill).not.toHaveBeenCalled();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
    expect(
      controlled.requests.filter(({ method }) => method === 'activation.configure'),
    ).toHaveLength(2);
    expect(controlled.client.activationCaptureEnabled).toBeNull();
    await controlled.client.stop();
    expect(controlled.client.activationCaptureEnabled).toBeNull();
  });

  it('clears effective activation when the helper closes during reconciliation', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const configuring = controlled.client.configureActivation(true, [
      shortcutFromLegacyActivation('Q', false),
    ]);
    const outcome = configuring.catch((error: unknown) => error);
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(baseline + 1),
    );

    controlled.close();
    await expect(outcome).resolves.toBeInstanceOf(Error);
    expect(controlled.client.activationCaptureEnabled).toBeNull();
    await controlled.client.stop();
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

  it('requires a disabled startup configuration round trip', async () => {
    const client = createClient('normal');
    await client.start();

    expect(client.readiness.status).toBe('ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('preserves the irreversible commit when abort precedes a delayed acknowledgement', async () => {
    const client = createClient('paste-delay');
    await client.start();
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = client.injectPaste(
      ACTIVATION_CONTEXT,
      EXPECTED_CLIPBOARD_SHA256,
      controller.signal,
      committed,
    );
    controller.abort();
    await expect(paste).resolves.toEqual({ submitted: true });
    expect(committed).toHaveBeenCalledOnce();
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
  });

  it('keeps transport healthy when a paste commit observer throws', async () => {
    const client = createClient('normal');
    await client.start();

    await expect(
      client.injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256, undefined, () => {
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
    const paste = controlled.client.injectPaste(
      ACTIVATION_CONTEXT,
      EXPECTED_CLIPBOARD_SHA256,
      undefined,
      () => ordering.push('committed'),
    );
    const observed = paste.then((result) => {
      ordering.push('settled');
      return result;
    });
    const request = latestRequest(controlled.requests, 'paste.inject');
    expect(request.params).toEqual({
      ...ACTIVATION_CONTEXT,
      expectedClipboardSha256: EXPECTED_CLIPBOARD_SHA256,
    });

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

  it('faults when a successful paste response arrives before its commit notification', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const committed = vi.fn();
    const paste = controlled.client
      .injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256, undefined, committed)
      .catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'paste.inject');

    controlled.emitStdout(
      Buffer.concat([
        encodeHelperFrame({
          jsonrpc: '2.0',
          id: request.id,
          result: { submitted: true },
        }),
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'paste.committed',
          params: { requestId: request.id },
        }),
      ]),
    );

    await expect(paste).resolves.toMatchObject({ code: 'transport-error' });
    expect(committed).not.toHaveBeenCalled();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
    controlled.close();
    await controlled.client.stop();
  });

  it('faults when a successful paste response omits its commit notification', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const committed = vi.fn();
    const paste = controlled.client
      .injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256, undefined, committed)
      .catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'paste.inject');

    controlled.emitResult(request.id, { submitted: true });

    await expect(paste).resolves.toMatchObject({ code: 'transport-error' });
    expect(committed).not.toHaveBeenCalled();
    expect(controlled.client.readiness.reason).toBe('malformed-response');
    controlled.close();
    await controlled.client.stop();
  });

  it('does not dispatch a queued successor after paste success precedes commitment', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.length;
    const committed = vi.fn();
    const paste = controlled.client
      .injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256, undefined, committed)
      .catch((error: unknown) => error);
    const queued = controlled.client.ping().catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'paste.inject');

    controlled.emitResult(request.id, { submitted: true });

    await expect(paste).resolves.toMatchObject({ code: 'transport-error' });
    await expect(queued).resolves.toMatchObject({ code: 'not-running' });
    expect(committed).not.toHaveBeenCalled();
    expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
      'paste.inject',
      'shutdown',
    ]);
    controlled.completeShutdown();
    await waitFor(() => controlled.client.readiness.status === 'unavailable');
  });

  it('reports pre-dispatch abort as uncommitted when the helper rejects dispatch', async () => {
    const client = createClient('paste-before-dispatch');
    await client.start();
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = client.injectPaste(
      ACTIVATION_CONTEXT,
      EXPECTED_CLIPBOARD_SHA256,
      controller.signal,
      committed,
    );
    controller.abort();
    await expect(paste).resolves.toMatchObject({ submitted: false });
    expect(committed).not.toHaveBeenCalled();
  });

  it('rejects queued paste and configuration before handling an invalid active schema', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.length;
    const active = controlled.client.request('ping', {}, 1_000).catch((error: unknown) => error);
    const queuedPaste = controlled.client
      .injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256)
      .catch((error: unknown) => error);
    const binding = shortcutFromLegacyActivation('Q', false);
    const queuedConfiguration = controlled.client
      .request('activation.configure', { enabled: true, bindings: [binding] }, 1_000)
      .catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'ping');

    controlled.emitResult(request.id, { ok: false, hookStatus: 'installed_unobserved' });

    await expect(active).resolves.toMatchObject({ code: 'transport-error' });
    await expect(queuedPaste).resolves.toMatchObject({ code: 'not-running' });
    await expect(queuedConfiguration).resolves.toMatchObject({ code: 'not-running' });
    // Invalid result validation releases only its own slot. The synchronous
    // fault decision drains both ordinary successors before reserved shutdown.
    expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
      'ping',
      'shutdown',
    ]);
    controlled.completeShutdown();
    await waitFor(() => controlled.client.readiness.status === 'unavailable');
  });

  it('keeps a malformed predecessor terminal while stop dispatches reserved shutdown', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.length;
    const predecessor = controlled.client
      .request('ping', {}, 1_000)
      .catch((error: unknown) => error);
    const request = latestRequest(controlled.requests, 'ping');
    const stopping = controlled.client.stop().catch((error: unknown) => error);

    controlled.emitResult(request.id, { ok: 'not-a-boolean', hookStatus: 'installed_unobserved' });

    await expect(predecessor).resolves.toMatchObject({ code: 'transport-error' });
    expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
      'ping',
      'shutdown',
    ]);
    controlled.completeShutdown();
    await expect(stopping).resolves.toMatchObject({ code: 'transport-error' });
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
  });

  it('serializes native request dispatch while continuing to parse notifications', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.length;
    const first = controlled.client.request('ping', {}, 1_000);
    const second = controlled.client.request('ping', {}, 1_000);

    expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual(['ping']);
    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'audio.input_devices_changed',
        params: {},
      }),
    );
    const firstRequest = controlled.requests.at(baseline);
    if (firstRequest === undefined) throw new Error('First request was not dispatched');
    controlled.emitResult(firstRequest.id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await expect(first).resolves.toMatchObject({ ok: true });
    expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
      'ping',
      'ping',
    ]);
    const secondRequest = controlled.requests.at(baseline + 1);
    if (secondRequest === undefined) throw new Error('Second request was not dispatched');
    controlled.emitResult(secondRequest.id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await expect(second).resolves.toMatchObject({ ok: true });
  });

  it('reserves shutdown dispatch beyond 256 saturated ordinary requests', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const baseline = controlled.requests.length;
    const outcomes = Array.from({ length: 256 }, () =>
      controlled.client.request('ping', {}, 2_000).catch((error: unknown) => error),
    );
    await expect(controlled.client.request('ping', {}, 2_000)).rejects.toMatchObject({
      code: 'request-capacity',
    });
    expect(controlled.requests.slice(baseline)).toHaveLength(1);

    const stopping = controlled.client.stop();
    await Promise.resolve();
    const dispatched = controlled.requests.at(baseline);
    if (dispatched === undefined) throw new Error('Saturated predecessor was not dispatched');
    controlled.emitResult(dispatched.id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await vi.waitFor(() =>
      expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
        'ping',
        'shutdown',
      ]),
    );
    controlled.completeShutdown();
    await expect(stopping).resolves.toBeUndefined();
    const settled = await Promise.all(outcomes);
    expect(settled.filter((value) => value instanceof Error)).toHaveLength(255);
  });

  it('starts the full native shutdown envelope only when shutdown is dispatched', async () => {
    const controlled = createControlledClient({
      nativeDrainEnvelopeMs: 80,
      predecessorDrainEnvelopeMs: 250,
    });
    await controlled.client.start();
    const predecessor = controlled.client
      .request('ping', {}, 1_000)
      .catch((error: unknown) => error);
    const predecessorRequest = latestRequest(controlled.requests, 'ping');
    const stopping = controlled.client.stop().catch((error: unknown) => error);

    await new Promise((resolveWait) => setTimeout(resolveWait, 120));
    expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    expect(controlled.kill).not.toHaveBeenCalled();

    controlled.emitResult(predecessorRequest.id, {
      ok: true,
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await expect(predecessor).resolves.toMatchObject({ ok: true });
    expect(controlled.requests.at(-1)?.method).toBe('shutdown');
    await new Promise((resolveWait) => setTimeout(resolveWait, 60));
    expect(controlled.kill).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(controlled.kill).toHaveBeenCalledOnce());
    controlled.close();
    await expect(stopping).resolves.toBeInstanceOf(Error);
  });

  it('parses a late ignored predecessor and final shutdown response from one chunk', async () => {
    const controlled = createControlledClient({
      nativeDrainEnvelopeMs: 500,
      predecessorDrainEnvelopeMs: 500,
    });
    await controlled.client.start();
    vi.useFakeTimers();
    try {
      const predecessor = controlled.client
        .request('ping', {}, 100)
        .catch((error: unknown) => error);
      const predecessorRequest = latestRequest(controlled.requests, 'ping');
      const stopping = controlled.client.stop();
      await vi.advanceTimersByTimeAsync(101);
      await expect(predecessor).resolves.toMatchObject({ code: 'request-timeout' });
      const shutdown = latestRequest(controlled.requests, 'shutdown');

      controlled.emitStdout(
        Buffer.concat([
          encodeHelperFrame({
            jsonrpc: '2.0',
            id: predecessorRequest.id,
            result: { ok: true, hookStatus: 'installed_unobserved', keyboardOwner: OWNER_SNAPSHOT },
          }),
          encodeHelperFrame({
            jsonrpc: '2.0',
            id: shutdown.id,
            result: { ownerDisposition: 'neutral' },
          }),
        ]),
      );
      controlled.closeSuccessfully();
      await expect(stopping).resolves.toBeUndefined();
      expect(controlled.kill).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('closes application writes and attempts bounded shutdown after compromised output', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 100 });
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
    await vi.waitFor(() => expect(controlled.kill).toHaveBeenCalledOnce());
    expect(controlled.writes.length).toBeGreaterThanOrEqual(writesBeforeFailure);
    expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    expect(await firstRejection).toMatchObject({ code: 'transport-error' });
    expect(await secondRejection).toMatchObject({ code: 'not-running' });
    controlled.close();
    await controlled.client.stop();
  });

  it('removes a pre-dispatch abort from the backpressured queue', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 100 });
    await controlled.client.start();
    controlled.blockNextWrite();
    const first = controlled.client.request('ping', {}, 1_000);
    const firstRejection = first.catch((error: unknown) => error);
    const controller = new AbortController();
    const committed = vi.fn();
    const paste = controlled.client.injectPaste(
      ACTIVATION_CONTEXT,
      EXPECTED_CLIPBOARD_SHA256,
      controller.signal,
      committed,
    );
    const writesBeforeAbort = controlled.writes.length;

    controller.abort();

    await expect(paste).rejects.toMatchObject({ name: 'AbortError' });
    controlled.emitDrain();
    expect(controlled.writes).toHaveLength(writesBeforeAbort);
    expect(committed).not.toHaveBeenCalled();
    controlled.emitStdout(Buffer.from([0, 0, 0, 1, 0x7b]));
    await vi.waitFor(() => expect(controlled.kill).toHaveBeenCalledOnce());
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
    const paste = controlled.client.injectPaste(
      ACTIVATION_CONTEXT,
      EXPECTED_CLIPBOARD_SHA256,
      undefined,
      committed,
    );

    const resetOutcome = controlled.client.resetSessionCapture().catch((error: unknown) => error);
    await expect(paste).rejects.toMatchObject({ code: 'not-running' });
    controlled.emitDrain();
    controlled.emitError(latestRequest(controlled.requests, 'ping').id);

    expect(controlled.requests.some(({ method }) => method === 'paste.inject')).toBe(false);
    expect(committed).not.toHaveBeenCalled();
    const stopping = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await expect(stopping).resolves.toBeUndefined();
    await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
    await expect(firstOutcome).resolves.toMatchObject({ code: 'rpc-error' });
  });

  it('does not publish a health snapshot after capture reset starts draining', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    const activationCalls = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const health = controlled.client.getPermissions();
    const healthOutcome = health.catch((error: unknown) => error);
    const resetOutcome = controlled.client.resetSessionCapture().catch((error: unknown) => error);

    controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
      accessibility: 'denied',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    });

    const stopping = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();
    await expect(stopping).resolves.toBeUndefined();
    await expect(healthOutcome).resolves.toMatchObject({ code: 'not-running' });
    expect(
      controlled.requests.filter(({ method }) => method === 'activation.configure'),
    ).toHaveLength(activationCalls);
    await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
  });

  it('establishes fault drain before a timed-out request can dispatch queued mutations', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 500 });
    await controlled.client.start();
    const baseline = controlled.requests.length;
    vi.useFakeTimers();
    try {
      const active = controlled.client.request('ping', {}, 100).catch((error: unknown) => error);
      const paste = controlled.client
        .injectPaste(ACTIVATION_CONTEXT, EXPECTED_CLIPBOARD_SHA256)
        .catch((error: unknown) => error);
      const configuration = controlled.client
        .request(
          'activation.configure',
          {
            enabled: true,
            bindings: [shortcutFromLegacyActivation('Q', false)],
          },
          1_000,
        )
        .catch((error: unknown) => error);

      await vi.advanceTimersByTimeAsync(101);
      await expect(active).resolves.toMatchObject({ code: 'request-timeout' });
      await expect(paste).resolves.toMatchObject({ code: 'not-running' });
      await expect(configuration).resolves.toMatchObject({ code: 'not-running' });
      expect(controlled.requests.slice(baseline).map(({ method }) => method)).toEqual([
        'ping',
        'shutdown',
      ]);

      controlled.completeShutdown();
      await vi.advanceTimersByTimeAsync(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not let an earlier request timeout force-kill graceful reset', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 500 });
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
      await vi.advanceTimersByTimeAsync(0);
      controlled.completeShutdown();
      await expect(stopping).resolves.toBeUndefined();
      await expect(resetOutcome).resolves.toMatchObject({ code: 'not-running' });
    } finally {
      vi.useRealTimers();
    }
  });

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
      controlled.emitResult(deniedPermissions.id, {
        accessibility: 'denied',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      const unavailablePing = latestRequest(controlled.requests, 'ping');
      controlled.emitResult(unavailablePing.id, {
        ok: true,
        hookStatus: 'unavailable',
        keyboardOwner: OWNER_SNAPSHOT,
      });
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
      controlled.emitResult(restoredPermissions.id, {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      const readyPing = latestRequest(controlled.requests, 'ping');
      controlled.emitResult(readyPing.id, {
        ok: true,
        hookStatus: 'installed_unobserved',
        keyboardOwner: OWNER_SNAPSHOT,
      });
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
      expect(controlled.requests.filter(({ method }) => method === 'permissions.get')).toHaveLength(
        permissionRequestsBeforeRefresh,
      );
      controlled.emitResult(staleEnable.id, {
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Z', false)],
      });
      expect(controlled.requests.filter(({ method }) => method === 'permissions.get')).toHaveLength(
        permissionRequestsBeforeRefresh + 1,
      );
      controlled.emitResult(latestRequest(controlled.requests, 'permissions.get').id, {
        accessibility: 'denied',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      });
      controlled.emitResult(latestRequest(controlled.requests, 'ping').id, {
        ok: true,
        hookStatus: 'unavailable',
        keyboardOwner: OWNER_SNAPSHOT,
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
      keyboardOwner: OWNER_SNAPSHOT,
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
    await vi.waitFor(() =>
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(true),
    );
    controlled.completeShutdown();
    expect(controlled.kill).not.toHaveBeenCalled();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'hook-fault',
    });
    await controlled.client.stop();
  });

  it('bounds restart when a killed child never confirms close', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();
    vi.useFakeTimers();
    try {
      const restarting = controlled.client.restart();
      const outcome = restarting.catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(1_501);
      await expect(outcome).resolves.toMatchObject({
        code: 'transport-error',
        message: 'Native helper restart could not confirm process exit',
      });
      expect(controlled.client.readiness.status).toBe('unavailable');
    } finally {
      vi.useRealTimers();
      controlled.close();
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
      nativeDrainEnvelopeMs: 500,
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
    await client.setSessionCapture('recording');

    await client.restart();
    await waitFor(() => launches >= 2 && client.readiness.status === 'ready');
    await expect(client.ping()).resolves.toMatchObject({ ok: true });
    expect(client.activationCaptureEnabled).toBe(true);
    await client.stop();
    expect(client.activationCaptureEnabled).toBeNull();
  });

  it('confirms capture is disabled before replaying retained activation on a fresh helper', async () => {
    const controlled = createControlledClient({
      deferStartupHealth: true,
      deferStartupCapture: true,
    });
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
      hookStatus: 'installed_unobserved',
      keyboardOwner: OWNER_SNAPSHOT,
    });
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'session.set_capture').params).toEqual({
        mode: 'off',
      }),
    );
    expect(controlled.requests.some(({ method }) => method === 'activation.configure')).toBe(false);
    expect(controlled.client.readiness.status).toBe('starting');
    controlled.emitResult(latestRequest(controlled.requests, 'session.set_capture').id, {
      mode: 'off',
    });
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: false,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      }),
    );
    controlled.emitResult(latestRequest(controlled.requests, 'activation.configure').id, {
      enabled: false,
      bindings: [shortcutFromLegacyActivation('Q', false)],
    });
    await vi.waitFor(() =>
      expect(latestRequest(controlled.requests, 'activation.configure').params).toEqual({
        enabled: true,
        bindings: [shortcutFromLegacyActivation('Q', false)],
      }),
    );
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
      nativeDrainEnvelopeMs: 500,
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
      nativeDrainEnvelopeMs: 500,
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
      nativeDrainEnvelopeMs: 500,
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
    await client.setSessionCapture('cancel-only');

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
            'mode' in params &&
            params.mode === 'off',
        ),
      ).toBe(true),
    );
    const writesBeforeAdmissionCheck = controlled.writes.length;
    await expect(controlled.client.ping()).rejects.toMatchObject({ code: 'not-running' });
    expect(controlled.writes).toHaveLength(writesBeforeAdmissionCheck);

    const stopping = controlled.client.stop();
    await vi.waitFor(() => expect(latestRequest(controlled.requests, 'shutdown')).toBeDefined());
    controlled.completeShutdown();

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
      nativeDrainEnvelopeMs: 500,
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
    const client = createClient('slow-shutdown', 500);
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

  it('routes validated input-device invalidations without exposing endpoint identifiers', async () => {
    const controlled = createControlledClient();
    const invalidated = vi.fn();
    const remove = controlled.client.subscribeInputDeviceInvalidations(invalidated);
    await controlled.client.start();

    controlled.emitStdout(
      Buffer.concat([
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            phase: 'down',
            profileId: 'general',
            shortcut: shortcutFromLegacyActivation('Z', false).shortcut,
            activationGeneration: 1,
            targetToken: null,
          },
        }),
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'audio.input_devices_changed',
          params: {},
        }),
      ]),
    );

    expect(invalidated).toHaveBeenCalledOnce();
    remove();
    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'audio.input_devices_changed',
        params: {},
      }),
    );
    expect(invalidated).toHaveBeenCalledOnce();
    expect(controlled.client.readiness.status).toBe('ready');
    controlled.close();
    await controlled.client.stop();
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

  it('treats a protocol-v7 helper rejection as a stable protocol mismatch', async () => {
    const client = createClient('protocol-v7');
    await client.start();
    await waitFor(() => client.readiness.status === 'incompatible');
    expect(client.readiness).toMatchObject({
      status: 'incompatible',
      reason: 'protocol-mismatch',
    });
  });

  it('rejects a mixed v7 activation shape after a v8 handshake', async () => {
    const controlled = createControlledClient();
    await controlled.client.start();

    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: 'general',
          shortcut: shortcutFromLegacyActivation('Z', false).shortcut,
        },
      }),
    );

    await vi.waitFor(() =>
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(true),
    );
    controlled.completeShutdown();
    expect(controlled.kill).not.toHaveBeenCalled();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
    await controlled.client.stop();
  });

  it('accepts a shared-prefix completion that races ahead of configuration success', async () => {
    const controlled = createControlledClient();
    const notifications: HelperNotification[] = [];
    controlled.client.subscribeNotifications((notification) => notifications.push(notification));
    await controlled.client.start();
    const shortcut = shortcutFromLegacyActivation('Z', false);

    const baseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const configuring = controlled.client.configureActivation(true, [shortcut]);
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(baseline + 1),
    );
    const request = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          ...shortcut,
          activationGeneration: 2,
          targetToken: null,
          heldMs: 10,
        },
      }),
    );
    controlled.emitResult(request.id, request.params);
    await configuring;

    expect(notifications).toHaveLength(1);
    expect(notifications.at(-1)).toMatchObject({
      method: 'activation.event',
      params: { phase: 'complete', activationGeneration: 2 },
    });
    expect(controlled.client.readiness.status).toBe('ready');
    expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(false);
    controlled.close();
    await controlled.client.stop();
  });

  it('preserves a replacement down across the delayed configuration response', async () => {
    const controlled = createControlledClient();
    const notifications: HelperNotification[] = [];
    controlled.client.subscribeNotifications((notification) => notifications.push(notification));
    await controlled.client.start();
    const shortcut = shortcutFromLegacyActivation('Z', false);
    const emitActivation = (phase: 'down' | 'up', generation: number) =>
      controlled.emitStdout(
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            phase,
            ...shortcut,
            activationGeneration: generation,
            targetToken: null,
          },
        }),
      );

    emitActivation('down', 1);
    const baseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const configuring = controlled.client.configureActivation(true, [shortcut]);
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(baseline + 1),
    );
    emitActivation('down', 2);
    expect(notifications).toHaveLength(1);
    const request = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitResult(request.id, request.params);
    await configuring;
    emitActivation('up', 2);

    expect(
      notifications.map((notification) =>
        notification.method === 'activation.event'
          ? notification.params.phase
          : notification.method,
      ),
    ).toEqual(['down', 'down', 'up']);
    expect(controlled.client.readiness.status).toBe('ready');
    controlled.close();
    await controlled.client.stop();
  });

  it('does not publish a deferred activation when configuration fails', async () => {
    const controlled = createControlledClient();
    const notifications: HelperNotification[] = [];
    controlled.client.subscribeNotifications((notification) => notifications.push(notification));
    await controlled.client.start();
    const shortcut = shortcutFromLegacyActivation('Z', false);
    const baseline = controlled.requests.filter(
      ({ method }) => method === 'activation.configure',
    ).length;
    const configuring = controlled.client.configureActivation(true, [shortcut]);
    await vi.waitFor(() =>
      expect(
        controlled.requests.filter(({ method }) => method === 'activation.configure'),
      ).toHaveLength(baseline + 1),
    );
    controlled.emitStdout(
      encodeHelperFrame({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          ...shortcut,
          activationGeneration: 2,
          targetToken: null,
          heldMs: 10,
        },
      }),
    );
    const request = latestRequest(controlled.requests, 'activation.configure');
    controlled.emitError(request.id, -32_050);

    await expect(configuring).rejects.toMatchObject({ code: 'rpc-error' });
    expect(notifications).toHaveLength(0);
    controlled.close();
    await controlled.client.stop();
  });

  it.each([
    ['an orphan up', [{ phase: 'up', activationGeneration: 1, targetToken: null }]],
    [
      'a reused generation',
      [
        { phase: 'complete', activationGeneration: 2, targetToken: null, heldMs: 10 },
        { phase: 'down', activationGeneration: 2, targetToken: null },
      ],
    ],
    [
      'a decreasing generation',
      [
        { phase: 'complete', activationGeneration: 2, targetToken: null, heldMs: 10 },
        { phase: 'complete', activationGeneration: 1, targetToken: null, heldMs: 10 },
      ],
    ],
    [
      'a mismatched target token on up',
      [
        { phase: 'down', activationGeneration: 1, targetToken: null },
        { phase: 'up', activationGeneration: 1, targetToken: 'different-target' },
      ],
    ],
  ] as const)('rejects %s in an otherwise valid v8 activation stream', async (_name, events) => {
    const controlled = createControlledClient();
    await controlled.client.start();

    for (const params of events) {
      controlled.emitStdout(
        encodeHelperFrame({
          jsonrpc: '2.0',
          method: 'activation.event',
          params: {
            profileId: 'general',
            shortcut: shortcutFromLegacyActivation('Z', false).shortcut,
            ...params,
          },
        }),
      );
    }

    await vi.waitFor(() =>
      expect(controlled.requests.some(({ method }) => method === 'shutdown')).toBe(true),
    );
    controlled.completeShutdown();
    expect(controlled.kill).not.toHaveBeenCalled();
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
    await controlled.client.stop();
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

  it.each([['forged collision text', 'forged-collision-diagnostic']] as const)(
    'does not trust %s from a process that exits with code 1',
    async (_label, scenario) => {
      const { client, launches, pids } = createCountedProcessClient(scenario);
      await client.start();
      await waitFor(() => launches() >= 2, 2_000);

      expect(launches()).toBeGreaterThanOrEqual(2);
      expect(new Set(pids()).size).toBeGreaterThanOrEqual(2);
      expect(client.nativeLaunchFailure).not.toBe('owner-singleton-collision');
      expect(client.readiness.reason).not.toBe('owner-singleton-collision');
    },
  );

  it('bounds repeated forged collision stderr with the normal crash-loop circuit', async () => {
    vi.useFakeTimers();
    let launches = 0;
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform: process.platform === 'win32' ? 'win32' : 'darwin',
      architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
      spawnHelper: () => {
        launches += 1;
        const stdin = new EventEmitter() as EventEmitter & {
          destroyed: boolean;
          writable: boolean;
          write: () => boolean;
        };
        const stdout = new EventEmitter();
        const stderr = new EventEmitter();
        const processEmitter = new EventEmitter() as EventEmitter & {
          stdin: typeof stdin;
          stdout: EventEmitter;
          stderr: EventEmitter;
          exitCode: number | null;
          signalCode: NodeJS.Signals | null;
          kill: () => boolean;
        };
        let closed = false;
        stdin.destroyed = false;
        stdin.writable = true;
        stdin.write = () => {
          if (!closed) {
            closed = true;
            queueMicrotask(() => {
              stderr.emit(
                'data',
                Buffer.from('talking-quill-helper: forged OWNER_SINGLETON_COLLISION\n'),
              );
              stderr.emit('end');
              stdout.emit('end');
              stdin.writable = false;
              processEmitter.exitCode = 1;
              processEmitter.emit('close', 1, null);
            });
          }
          return true;
        };
        processEmitter.stdin = stdin;
        processEmitter.stdout = stdout;
        processEmitter.stderr = stderr;
        processEmitter.exitCode = null;
        processEmitter.signalCode = null;
        processEmitter.kill = () => true;
        return processEmitter as unknown as ChildProcessWithoutNullStreams;
      },
    });
    clients.push(client);
    try {
      await client.start();
      for (const [index, delay] of [250, 1_000, 4_000, 15_000].entries()) {
        await vi.advanceTimersByTimeAsync(delay);
        await vi.waitFor(() => expect(launches).toBe(index + 2));
      }
      expect(launches).toBe(5);
      expect(client.readiness).toMatchObject({
        status: 'unavailable',
        reason: 'crash-loop',
      });
    } finally {
      await client.stop();
      vi.useRealTimers();
    }
  });

  it('still relaunches a process after an ordinary transient exit', async () => {
    const { client, launches } = createCountedProcessClient('exit');
    await client.start();
    await waitFor(() => launches() >= 2, 2_000);

    expect(launches()).toBeGreaterThanOrEqual(2);
    expect(client.readiness.reason).not.toBe('owner-singleton-collision');
  });

  it('probes a crash loop at bounded intervals and recovers on a later half-open launch', async () => {
    vi.useFakeTimers();
    let launches = 0;
    const recoveredRequests: {
      readonly id: number;
      readonly method: string;
      readonly params: unknown;
    }[] = [];
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform: process.platform === 'win32' ? 'win32' : 'darwin',
      architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
      spawnHelper: () => {
        launches += 1;
        if (launches <= 6) throw new Error('helper launch failed');
        return createAutomaticChild(recoveredRequests);
      },
    });
    clients.push(client);
    try {
      await client.start();
      for (const [index, delay] of [250, 1_000, 4_000, 15_000].entries()) {
        await vi.advanceTimersByTimeAsync(delay);
        await vi.waitFor(() => expect(launches).toBe(index + 2));
      }
      expect(launches).toBe(5);
      expect(client.readiness).toMatchObject({
        status: 'unavailable',
        reason: 'crash-loop',
      });

      await vi.advanceTimersByTimeAsync(119_999);
      expect(launches).toBe(5);
      await vi.advanceTimersByTimeAsync(1);
      await vi.waitFor(() => expect(launches).toBe(6));
      await vi.waitFor(() => expect(client.readiness.reason).toBe('crash-loop'));

      await vi.advanceTimersByTimeAsync(119_999);
      expect(launches).toBe(6);
      await vi.advanceTimersByTimeAsync(1);
      await vi.waitFor(() => expect(client.readiness.status).toBe('ready'));
      expect(launches).toBe(7);
      expect(
        recoveredRequests.some(
          ({ method, params }) =>
            method === 'session.set_capture' &&
            typeof params === 'object' &&
            params !== null &&
            'mode' in params &&
            params.mode === 'off',
        ),
      ).toBe(true);
    } finally {
      await client.stop();
      vi.useRealTimers();
    }
  });

  it('cancels a pending half-open probe when stopped', async () => {
    vi.useFakeTimers();
    let launches = 0;
    const client = new HelperClient({
      executablePath: process.execPath,
      expectedHelperVersion: '1.0.0',
      platform: process.platform === 'win32' ? 'win32' : 'darwin',
      architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
      spawnHelper: () => {
        launches += 1;
        throw new Error('helper launch failed');
      },
    });
    clients.push(client);
    try {
      await client.start();
      for (let attempt = 1; attempt < 5; attempt += 1) await client.restart();
      expect(client.readiness.reason).toBe('crash-loop');

      await client.stop();
      await vi.advanceTimersByTimeAsync(10 * 120_000);

      expect(launches).toBe(5);
      expect(client.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
    } finally {
      await client.stop();
      vi.useRealTimers();
    }
  });

  it('classifies the first malformed frame as stable and launches exactly once', async () => {
    const controlled = createControlledClient({ deferInitialize: true, nativeDrainEnvelopeMs: 10 });
    const starting = controlled.client.start();
    await waitFor(() => controlled.requests.some(({ method }) => method === 'initialize'));
    const malformed = Buffer.from('{', 'utf8');
    const prefix = Buffer.alloc(4);
    prefix.writeUInt32BE(malformed.length);
    controlled.emitStdout(Buffer.concat([prefix, malformed]));
    await starting;
    await waitFor(() => controlled.client.readiness.reason === 'malformed-response');
    controlled.closeSuccessfully();
    await new Promise((resolveWait) => setTimeout(resolveWait, 400));
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness).toMatchObject({
      status: 'unavailable',
      reason: 'malformed-response',
    });
  });

  it('does not relaunch after the first malformed frame on an authoritative ready session', async () => {
    const controlled = createControlledClient({ nativeDrainEnvelopeMs: 10 });
    await controlled.client.start();
    expect(controlled.client.readiness.status).toBe('ready');
    const malformed = Buffer.from('{', 'utf8');
    const prefix = Buffer.alloc(4);
    prefix.writeUInt32BE(malformed.length);
    controlled.emitStdout(Buffer.concat([prefix, malformed]));
    await waitFor(() => controlled.client.readiness.reason === 'malformed-response');
    controlled.closeSuccessfully();
    await new Promise((resolveWait) => setTimeout(resolveWait, 400));
    expect(controlled.launches()).toBe(1);
    expect(controlled.client.readiness.reason).toBe('malformed-response');
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
