import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HelperClient } from '../../app/src/main/helper/helper-client';

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
