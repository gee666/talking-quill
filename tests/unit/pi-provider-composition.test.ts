import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import {
  PiProvider,
  PI_RPC_READY_TTL_MS,
  type PiCliIdentity,
  type PiProviderOptions,
  type SpawnPi,
} from '../../app/src/main/providers/pi';
import { ProviderError } from '../../app/src/main/providers/errors';
import {
  PI_RPC_PROTOCOL_VERSION,
  PI_RPC_REQUIRED_SAFETY_FLAGS,
} from '../../app/src/main/providers/pi-rpc-operation';

const identity: PiCliIdentity = Object.freeze({
  canonicalPath: '/opt/pi',
  packageVersion: 'unsupported-rpc-version',
  safetyFlags: PI_RPC_REQUIRED_SAFETY_FLAGS,
  fileIdentity: { dev: '1', ino: '2', size: 3, mtimeMs: 4 },
});
const invocation = {
  config: { providerId: 'pi' as const, modelId: 'fixture/model', thinking: 'high' as const },
  credential: null,
};
const signal = (): AbortSignal => new AbortController().signal;
const catalog = 'provider  model  context  images\nfixture  model  8K  no\n';

function printSpawn(output: () => { stdout: string; stderr?: string; code?: number }) {
  return vi.fn<SpawnPi>(() => {
    const child = new EventEmitter() as ChildProcessWithoutNullStreams;
    const stdin = new PassThrough();
    const stdout = new PassThrough();
    const stderr = new PassThrough();
    Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
    stdin.once('finish', () => {
      const result = output();
      stdout.end(result.stdout);
      stderr.end(result.stderr ?? '');
      child.emit('close', result.code ?? 0, null);
    });
    return child;
  });
}

function options(overrides: PiProviderOptions = {}): PiProviderOptions {
  return {
    platform: 'linux',
    environment: {},
    resolveCli: () => Promise.resolve(identity),
    ...overrides,
  };
}

describe('Pi provider module composition', () => {
  it('keeps the injected clock and refresh flag authoritative for the catalog cache', async () => {
    let now = 1_000;
    const spawnPi = printSpawn(() => ({ stdout: catalog }));
    const provider = new PiProvider(options({ spawnPi, now: () => now }));
    const first = await provider.listModels(invocation, signal());
    expect(await provider.listModels(invocation, signal())).toBe(first);
    now += 5 * 60_000 - 1;
    expect(await provider.listModels(invocation, signal())).toBe(first);
    expect(spawnPi).toHaveBeenCalledTimes(1);
    now += 1;
    const expired = await provider.listModels(invocation, signal());
    expect(expired).not.toBe(first);
    expect(spawnPi).toHaveBeenCalledTimes(2);
    expect(await provider.listModels({ ...invocation, refreshModels: true }, signal())).not.toBe(
      expired,
    );
    expect(spawnPi).toHaveBeenCalledTimes(3);
  });

  it('invalidates the catalog when revalidation fails even if discovery returns the same identity', async () => {
    let stale = false;
    const spawnPi = printSpawn(() => ({ stdout: catalog }));
    const resolveCli = vi.fn(() => Promise.resolve(identity));
    const provider = new PiProvider(
      options({
        spawnPi,
        resolveCli,
        revalidateCli: () => {
          if (!stale) return Promise.resolve();
          stale = false;
          return Promise.reject(new Error('temporarily stale'));
        },
      }),
    );
    const first = await provider.listModels(invocation, signal());
    stale = true;
    expect(await provider.listModels(invocation, signal())).not.toBe(first);
    expect(resolveCli).toHaveBeenCalledTimes(2);
    expect(spawnPi).toHaveBeenCalledTimes(2);
  });

  it('clears the catalog after print failure without discarding a valid executable', async () => {
    let fail = false;
    const spawnPi = printSpawn(() =>
      fail ? { stdout: '', stderr: 'authentication failed', code: 1 } : { stdout: catalog },
    );
    const resolveCli = vi.fn(() => Promise.resolve(identity));
    const provider = new PiProvider(options({ spawnPi, resolveCli }));
    const first = await provider.listModels(invocation, signal());
    fail = true;
    await expect(
      provider.cleanTranscript(invocation, { input: 'private input' }, signal()),
    ).rejects.toMatchObject({ code: 'AUTHENTICATION_FAILED' });
    fail = false;
    expect(await provider.listModels(invocation, signal())).not.toBe(first);
    expect(resolveCli).toHaveBeenCalledTimes(1);
    expect(spawnPi).toHaveBeenCalledTimes(3);
  });

  it('freezes print fallback configuration and consumes the lease exactly once', async () => {
    const spawnPi = printSpawn(() => ({ stdout: ' cleaned ' }));
    const provider = new PiProvider(options({ spawnPi }));
    const config = { ...invocation.config, piExtensionSources: [] as string[] };
    const prepared = await provider.prepareCompletion({ ...invocation, config }, signal());
    if (prepared === null) throw new Error('expected prepared completion');
    config.modelId = 'changed/model';
    config.piExtensionSources.push('./must-not-resolve.ts');
    await expect(prepared.complete({ input: 'private input' }, signal())).resolves.toBe('cleaned');
    await prepared.closed;
    await expect(prepared.complete({ input: 'duplicate' }, signal())).rejects.toMatchObject({
      code: 'INVALID_CONFIG',
    });
    expect(spawnPi).toHaveBeenCalledTimes(1);
    expect(spawnPi.mock.calls[0]?.[1]).toEqual([
      '-p',
      '--model',
      'fixture/model',
      '--thinking',
      'high',
      ...PI_RPC_REQUIRED_SAFETY_FLAGS,
    ]);
  });

  it('preserves the prewarm hook and blocks foreground spawns after unconfirmed cleanup', async () => {
    const failure = new ProviderError('PI_LAUNCH_FAILED');
    const prewarmRpcOperation = vi.fn<NonNullable<PiProviderOptions['prewarmRpcOperation']>>(() =>
      Promise.reject(failure),
    );
    const spawnPi = printSpawn(() => ({ stdout: catalog }));
    const provider = new PiProvider(
      options({
        spawnPi,
        prewarmRpcOperation,
        resolveCli: () => Promise.resolve({ ...identity, packageVersion: PI_RPC_PROTOCOL_VERSION }),
      }),
    );
    await expect(provider.prepareCompletion(invocation, signal())).rejects.toBe(failure);
    expect(prewarmRpcOperation).toHaveBeenCalledOnce();
    expect(prewarmRpcOperation.mock.calls[0]?.[0]).toMatchObject({
      expected: { provider: 'fixture', model: 'model', thinking: 'high' },
      explicitExtensions: [],
      spawnPi,
      platform: 'linux',
      environment: {},
      timeoutMs: 120_000,
    });
    await expect(provider.listModels(invocation, signal())).rejects.toBe(failure);
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it('passes the normalized interactive Windows home to installed extension resolution', async () => {
    const paths: string[] = [];
    const provider = new PiProvider(
      options({
        platform: 'win32',
        environment: { UserProfile: 'C:\\service-home' },
        interactiveHome: 'C:\\interactive-home',
        workingDirectory: 'C:\\app',
        canonicalizeExtensionPath: (path) => {
          paths.push(path);
          return Promise.reject(new Error('missing installation'));
        },
      }),
    );
    await expect(
      provider.listModels(
        {
          config: { providerId: 'pi', piExtensionSources: ['npm:trusted-extension'] },
          credential: null,
        },
        signal(),
      ),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    expect(paths).toEqual(['C:\\interactive-home\\.pi\\agent\\npm']);
  });

  it('retains ready TTL validation through the original provider export', () => {
    expect(PI_RPC_READY_TTL_MS).toBe(15_000);
    for (const rpcReadyTtlMs of [0, -1, 120_001, Number.NaN, 1.5]) {
      expect(() => new PiProvider(options({ rpcReadyTtlMs }))).toThrow(ProviderError);
    }
  });
});
