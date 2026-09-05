import { describe, expect, it, vi } from 'vitest';
import {
  HelperClient,
  HelperClientError,
  type HelperClientOptions,
} from '../../app/src/main/helper/helper-client';
import * as helper from '../../app/src/main/helper';

function createClient(): HelperClient {
  const options: HelperClientOptions = {
    executablePath: process.execPath,
    expectedHelperVersion: '1.0.0',
    platform: process.platform === 'win32' ? 'win32' : 'darwin',
    architecture: process.arch === 'arm64' ? 'arm64' : 'x64',
    spawnHelper: () => {
      throw new Error('These API tests must not launch a helper');
    },
  };
  return new HelperClient(options);
}

describe('HelperClient facade compatibility', () => {
  it('retains the entry-point identities and keeps runtime operations private', () => {
    expect(helper.HelperClient).toBe(HelperClient);
    expect(helper.HelperClientError).toBe(HelperClientError);
    expect(Reflect.ownKeys(createClient())).toEqual([]);
    expect(Object.getOwnPropertyNames(HelperClient.prototype).sort()).toEqual(
      [
        'constructor',
        'readiness',
        'nativeLaunchFailure',
        'activationCaptureEnabled',
        'sessionKeyCaptureAvailable',
        'subscribeReadiness',
        'subscribeNotifications',
        'subscribeInputDeviceInvalidations',
        'start',
        'restart',
        'stop',
        'configureActivation',
        'beginPhysicalObservation',
        'samplePhysicalObservation',
        'endPhysicalObservation',
        'setSessionCapture',
        'resetSessionCapture',
        'injectPaste',
        'getFrontApp',
        'getPermissions',
        'getRuntimeObservability',
        'recordObservationAccepted',
        'prepareOwnerMaintenance',
        'ping',
        'request',
      ].sort(),
    );
  });

  it('isolates readiness state and listeners between client instances', async () => {
    const first = createClient();
    const second = createClient();
    const originalSecondReadiness = second.readiness;
    const firstListener = vi.fn();
    const secondListener = vi.fn();
    first.subscribeReadiness(() => {
      throw new Error('Consumer failure');
    });
    const unsubscribe = first.subscribeReadiness(firstListener);
    second.subscribeReadiness(secondListener);

    await first.stop();
    expect(firstListener).toHaveBeenCalledExactlyOnceWith(first.readiness);
    expect(first.readiness).toMatchObject({ status: 'stopped', reason: 'shutdown' });
    expect(Object.isFrozen(first.readiness)).toBe(true);
    expect(second.readiness).toBe(originalSecondReadiness);
    expect(secondListener).not.toHaveBeenCalled();

    unsubscribe();
    await first.stop();
    await second.stop();
    expect(firstListener).toHaveBeenCalledTimes(1);
    expect(secondListener).toHaveBeenCalledExactlyOnceWith(second.readiness);
  });

  it('still routes convenience methods through the public request method', async () => {
    const client = createClient();
    const failure = new Error('Overridden request');
    const request = vi.spyOn(client, 'request').mockRejectedValue(failure);
    await expect(client.ping()).rejects.toBe(failure);
    await expect(client.getFrontApp()).rejects.toBe(failure);
    const context = { activationGeneration: 1, targetToken: 'opaque-target' };
    const signal = new AbortController().signal;
    const committed = vi.fn();
    await expect(client.injectPaste(context, 'a'.repeat(64), signal, committed)).rejects.toBe(
      failure,
    );
    expect(request.mock.calls).toEqual([
      ['ping', {}],
      ['front_app.get', {}],
      [
        'paste.inject',
        { ...context, expectedClipboardSha256: 'a'.repeat(64) },
        3_000,
        signal,
        committed,
      ],
    ]);
  });

  it('returns a rejected promise for invalid maintenance deadlines before session checks', async () => {
    const client = createClient();
    const operation = client.prepareOwnerMaintenance(
      { operation: 'uninstall', transactionId: 'transaction-1', sourceBuildId: 'source-build' },
      0,
    );
    expect(operation).toBeInstanceOf(Promise);
    await expect(operation).rejects.toBeInstanceOf(HelperClientError);
    await expect(operation).rejects.toMatchObject({ code: 'request-timeout' });
    expect(client.readiness.status).toBe('starting');
  });

  it('preserves cancellation precedence when no helper is running', async () => {
    const client = createClient();
    const abort = new AbortController();
    abort.abort();
    await expect(client.request('ping', {}, 3_000, abort.signal)).rejects.toMatchObject({
      name: 'AbortError',
    });
    await expect(client.ping()).rejects.toMatchObject({ code: 'not-running' });
  });
});
