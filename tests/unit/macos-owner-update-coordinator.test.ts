import { createHash, createHmac } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createTestDirectory } from '../helpers/temp';
import {
  awaitAuthenticatedRollbackOrTerminate,
  boundedTerminate,
  supervisePreReadyStatus,
  superviseUninstallStatus,
  validateInstalledMacosMaintenanceCapability,
  validateMacosReplacementIdentity,
  waitForTerminalExit,
} from '../../app/src/main/info/macos-owner-update-coordinator';

const owned: string[] = [];
afterEach(async () => {
  await Promise.all(owned.splice(0).map((path) => rm(path, { recursive: true, force: true })));
});

function fakeChild() {
  const child = new EventEmitter() as EventEmitter & {
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    kill: ReturnType<typeof vi.fn>;
  };
  child.exitCode = null;
  child.signalCode = null;
  child.kill = vi.fn((signal?: NodeJS.Signals) => {
    child.signalCode = signal ?? 'SIGTERM';
    child.emit('exit', null, child.signalCode);
    return true;
  });
  return child;
}

const handoff = '11'.repeat(32);
const transaction = '22'.repeat(32);

function authenticatedStatus(state: string): string {
  const mac = createHmac('sha256', Buffer.from(handoff, 'hex'))
    .update('talking-quill/macos-finalizer-status/v1\0')
    .update(transaction)
    .update(state)
    .digest('hex');
  return `${JSON.stringify({ version: 1, transaction, state, mac })}\n`;
}

describe('macOS installed maintenance capability', () => {
  async function fixture() {
    const root = await createTestDirectory('macos-maintenance-capability');
    owned.push(root);
    const installedApp = join(root, 'Talking Quill.app');
    const resources = join(installedApp, 'Contents', 'Resources');
    const macos = join(installedApp, 'Contents', 'MacOS');
    const helperExecutable = join(macos, 'talking-quill-helper');
    const bridge = join(macos, 'talking-quill-macos-service-bridge');
    await mkdir(resources, { recursive: true });
    await mkdir(macos, { recursive: true });
    await writeFile(helperExecutable, 'helper');
    await writeFile(bridge, 'bridge');
    await writeFile(
      join(resources, 'keyboard-owner-installed-v1'),
      'talking-quill-keyboard-owner-v1\n',
    );
    await writeFile(
      join(resources, 'keyboard-owner-r5m.json'),
      JSON.stringify({
        releaseBuildDigest: '11'.repeat(32),
        gateway: { executableSha256: '22'.repeat(32) },
        owner: { executableSha256: '33'.repeat(32) },
        bridge: { executableSha256: '44'.repeat(32) },
        gatewayReleasePolicy: '',
      }),
    );
    return { installedApp, helperExecutable, bridge, resources };
  }

  it('accepts a complete owner, bridge and helper capability', async () => {
    const value = await fixture();
    await expect(
      validateInstalledMacosMaintenanceCapability({
        ...value,
        validateHelper: () => Promise.resolve(),
      }),
    ).resolves.toBeUndefined();
  });

  it.each(['missing-owner-marker', 'damaged-owner-marker', 'missing-bridge', 'invalid-helper'])(
    'disables maintenance for %s',
    async (fault) => {
      const value = await fixture();
      if (fault === 'missing-owner-marker') {
        await rm(join(value.resources, 'keyboard-owner-installed-v1'));
      } else if (fault === 'damaged-owner-marker') {
        await writeFile(join(value.resources, 'keyboard-owner-installed-v1'), 'damaged\n');
      } else if (fault === 'missing-bridge') {
        await rm(value.bridge);
      }
      await expect(
        validateInstalledMacosMaintenanceCapability({
          ...value,
          validateHelper:
            fault === 'invalid-helper'
              ? () => Promise.reject(new Error('invalid helper'))
              : () => Promise.resolve(),
        }),
      ).rejects.toThrow(/maintenance (?:capability|marker)/u);
    },
  );
});

describe('macOS replacement outer identity', () => {
  const source = {
    releaseBuildDigest: '11'.repeat(32),
    gateway: { executableSha256: '12'.repeat(32) },
    owner: { executableSha256: '13'.repeat(32) },
    bridge: { executableSha256: '14'.repeat(32) },
    gatewayReleasePolicy: '',
  };
  const target = {
    releaseBuildDigest: '21'.repeat(32),
    gateway: { executableSha256: '22'.repeat(32) },
    owner: { executableSha256: '23'.repeat(32) },
    bridge: { executableSha256: '24'.repeat(32) },
    gatewayReleasePolicy: '',
  };
  const roles = [
    {
      role: 'gateway' as const,
      path: 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
      sha256: target.gateway.executableSha256,
      suppressionCapable: false,
    },
    {
      role: 'owner' as const,
      path: 'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
      sha256: target.owner.executableSha256,
      suppressionCapable: true,
    },
    {
      role: 'authority' as const,
      path: 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
      sha256: target.bridge.executableSha256,
      suppressionCapable: false,
    },
  ];
  const designatedRequirement =
    'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"';
  const outerIdentity = {
    mode: 'certificate' as const,
    leafCertificateSha256: '41'.repeat(32),
    identifier: 'com.talkingquill.app',
    teamIdentifier: null,
    designatedRequirement,
    designatedRequirementSha256: createHash('sha256').update(designatedRequirement).digest('hex'),
  };
  const identity = {
    schemaVersion: 1 as const,
    version: '1.2.3',
    platform: 'mac' as const,
    architecture: 'x64' as const,
    ownerMode: 'local-unsigned-enabled' as const,
    packageMode: 'update' as const,
    sourceCommit: '51'.repeat(20),
    sourceTree: '52'.repeat(20),
    releaseBuildDigest: target.releaseBuildDigest,
    packageLayoutDigest: '31'.repeat(32),
    packageSha256: '32'.repeat(32),
    channel: 'latest-x64-mac',
    roles,
    outerIdentity,
    predecessor: {
      platform: 'mac' as const,
      architecture: 'x64' as const,
      version: '1.2.2',
      releaseBuildDigest: source.releaseBuildDigest,
      gatewaySha256: source.gateway.executableSha256,
      ownerSha256: source.owner.executableSha256,
    },
    transactionBinding: 'source-target-package-sha256-v1' as const,
  };
  const packageMetadata = {
    schemaVersion: 1,
    kind: 'talking-quill-local-owner-release',
    version: identity.version,
    platform: identity.platform,
    architecture: identity.architecture,
    ownerMode: identity.ownerMode,
    sourceCommit: identity.sourceCommit,
    sourceTree: identity.sourceTree,
    releaseBuildDigest: identity.releaseBuildDigest,
    packageLayoutDigest: identity.packageLayoutDigest,
    roles,
    outerIdentity,
    predecessor: identity.predecessor,
    update: {
      channel: identity.channel,
      payload: 'zip',
      transactionBinding: identity.transactionBinding,
      maintenanceInstaller: 'macos-owner-finalizer',
    },
  };

  it('rejects missing outer identity for update and rollback candidates', () => {
    expect(() =>
      validateMacosReplacementIdentity({
        identity: undefined,
        archiveSha256: identity.packageSha256,
        packageMetadata,
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toThrow('identity is required');
  });

  it('rejects an altered-JS archive whose outer hash no longer matches', () => {
    expect(() =>
      validateMacosReplacementIdentity({
        identity,
        archiveSha256: 'ff'.repeat(32),
        packageMetadata,
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toThrow('complete macOS artifact');
  });

  it('rejects an attacker-updated adjacent sidecar that does not match embedded signer metadata', () => {
    expect(() =>
      validateMacosReplacementIdentity({
        identity: {
          ...identity,
          packageSha256: 'aa'.repeat(32),
          outerIdentity: {
            ...outerIdentity,
            designatedRequirement: 'identifier "com.talkingquill.app" and anchor attacker',
            designatedRequirementSha256: 'bb'.repeat(32),
          },
        },
        archiveSha256: 'aa'.repeat(32),
        packageMetadata,
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toThrow('complete macOS artifact');
  });

  it('rejects altered resources whose embedded release metadata changed', () => {
    expect(() =>
      validateMacosReplacementIdentity({
        identity,
        archiveSha256: identity.packageSha256,
        packageMetadata: { ...packageMetadata, packageLayoutDigest: 'ee'.repeat(32) },
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toThrow('complete macOS artifact');
  });

  it('rejects package metadata with different embedded source provenance', () => {
    expect(() =>
      validateMacosReplacementIdentity({
        identity,
        archiveSha256: identity.packageSha256,
        packageMetadata: { ...packageMetadata, sourceTree: 'ff'.repeat(20) },
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toThrow('complete macOS artifact');
  });

  it('accepts the exact archive hash, release metadata and predecessor', () => {
    expect(
      validateMacosReplacementIdentity({
        identity,
        archiveSha256: identity.packageSha256,
        packageMetadata,
        source,
        target,
        expectedArchitecture: 'x64',
      }),
    ).toEqual(identity);
  });
});

describe('macOS owner finalizer adversarial pre-ready supervision', () => {
  it.each(['malformed status', 'authentication mismatch', 'status pipe failure', 'early exit'])(
    'cancels and boundedly terminates on %s',
    async (fault) => {
      const child = fakeChild();
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const supervised = supervisePreReadyStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        10,
        20,
      );
      if (fault === 'malformed status') status.write('{not-json}\n');
      else if (fault === 'authentication mismatch')
        status.write(
          `${JSON.stringify({ version: 1, transaction, state: 'error', mac: '00'.repeat(32) })}\n`,
        );
      else if (fault === 'status pipe failure')
        status.emit('error', new Error('injected read failure'));
      else child.emit('exit', 70, null);
      await expect(supervised).rejects.toThrow();
      expect(cancellation.writableEnded).toBe(true);
      if (fault !== 'early exit') expect(child.kill).toHaveBeenCalled();
    },
  );

  it('accepts a required authenticated handoff buffered before recorded child exit', async () => {
    const child = fakeChild();
    const status = new PassThrough();
    const cancellation = new PassThrough();
    status.write(authenticatedStatus('ready'));
    child.exitCode = 0;
    await expect(
      supervisePreReadyStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        1_000,
        1_000,
      ),
    ).resolves.toBe('ready');
    expect(status.listenerCount('data')).toBe(0);
    expect(child.listenerCount('exit')).toBe(0);
  });

  it('allows authenticated status delivered after child exit but before pipe EOF', async () => {
    const child = fakeChild();
    const status = new PassThrough();
    const cancellation = new PassThrough();
    const supervised = supervisePreReadyStatus(
      child as never,
      status,
      cancellation,
      handoff,
      transaction,
      100,
      20,
    );
    child.exitCode = 70;
    child.emit('exit', 70, null);
    setTimeout(() => status.end(authenticatedStatus('ready')), 0);
    await expect(supervised).resolves.toBe('ready');
  });

  it('fails promptly when child exit is followed by status EOF', async () => {
    const child = fakeChild();
    const status = new PassThrough();
    const supervised = supervisePreReadyStatus(
      child as never,
      status,
      new PassThrough(),
      handoff,
      transaction,
      1_000,
      20,
    );
    child.exitCode = 70;
    child.emit('exit', 70, null);
    status.end();
    await expect(supervised).rejects.toThrow('exited');
  });

  it('keeps a leaked status writer bounded after child exit', async () => {
    const child = fakeChild();
    const started = Date.now();
    const supervised = supervisePreReadyStatus(
      child as never,
      new PassThrough(),
      new PassThrough(),
      handoff,
      transaction,
      15,
      20,
    );
    child.exitCode = 70;
    child.emit('exit', 70, null);
    await expect(supervised).rejects.toThrow('deadline');
    expect(Date.now() - started).toBeGreaterThanOrEqual(10);
    expect(Date.now() - started).toBeLessThan(100);
  });

  it.each(['partial', 'forged'] as const)(
    'rejects %s status delivered after child exit before EOF',
    async (fault) => {
      const child = fakeChild();
      const status = new PassThrough();
      const supervised = supervisePreReadyStatus(
        child as never,
        status,
        new PassThrough(),
        handoff,
        transaction,
        100,
        20,
      );
      child.exitCode = 70;
      child.emit('exit', 70, null);
      setTimeout(
        () =>
          status.end(
            fault === 'partial'
              ? '{"version":1'
              : `${JSON.stringify({ version: 1, transaction, state: 'ready', mac: '00'.repeat(32) })}\n`,
          ),
        0,
      );
      await expect(supervised).rejects.toThrow(fault === 'forged' ? 'authenticated' : 'exited');
    },
  );

  it.each(['before', 'during'] as const)(
    'fails initial handoff promptly when the child exits %s status listener installation',
    async (timing) => {
      const child = fakeChild();
      if (timing === 'before') {
        child.exitCode = 70;
      } else {
        const originalOnce = child.once.bind(child);
        child.once = ((event: string, listener: (...arguments_: unknown[]) => void) => {
          const result = originalOnce(event, listener);
          if (event === 'exit') child.exitCode = 70;
          return result;
        }) as typeof child.once;
      }
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const started = Date.now();
      const supervised = supervisePreReadyStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        1_000,
        1_000,
      );
      status.end();
      await expect(supervised).rejects.toThrow('exited');
      expect(Date.now() - started).toBeLessThan(100);
      expect(status.listenerCount('data')).toBe(0);
      expect(status.listenerCount('error')).toBe(0);
      expect(child.listenerCount('exit')).toBe(0);
    },
  );

  it.each(['before', 'during'] as const)(
    'fails uninstall terminal supervision promptly when exit occurs %s listener installation',
    async (timing) => {
      const child = fakeChild();
      let exitInstallations = 0;
      if (timing === 'during') {
        const originalOnce = child.once.bind(child);
        child.once = ((event: string, listener: (...arguments_: unknown[]) => void) => {
          const result = originalOnce(event, listener);
          if (event === 'exit' && ++exitInstallations === 2) child.exitCode = 70;
          return result;
        }) as typeof child.once;
      }
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const removeInstalledApp = vi.fn(() => {
        if (timing === 'before') child.exitCode = 70;
        status.end();
        return Promise.resolve();
      });
      const started = Date.now();
      const supervised = superviseUninstallStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        removeInstalledApp,
        1_000,
        1_000,
        10,
        1_000,
      );
      status.write(authenticatedStatus('uninstall_ready'));
      await expect(supervised).rejects.toThrow('exited');
      expect(Date.now() - started).toBeLessThan(100);
      expect(removeInstalledApp).toHaveBeenCalledOnce();
      expect(status.listenerCount('data')).toBe(0);
      expect(status.listenerCount('error')).toBe(0);
      expect(child.listenerCount('exit')).toBe(0);
    },
  );

  it('closes an exit that fires while rollback listeners are installed', async () => {
    const child = fakeChild();
    const originalOnce = child.once.bind(child);
    child.once = ((event: string, listener: (...arguments_: unknown[]) => void) => {
      const result = originalOnce(event, listener);
      if (event === 'exit') child.exitCode = 70;
      return result;
    }) as typeof child.once;
    const started = Date.now();
    await awaitAuthenticatedRollbackOrTerminate(
      child as never,
      new PassThrough(),
      handoff,
      transaction,
      1_000,
    );
    expect(Date.now() - started).toBeLessThan(100);
    expect(child.kill).not.toHaveBeenCalled();
  });

  it.each([
    [
      'terminal wait',
      (child: ReturnType<typeof fakeChild>) => waitForTerminalExit(child as never, 1_000),
    ],
    [
      'bounded termination',
      (child: ReturnType<typeof fakeChild>) => boundedTerminate(child as never, 1_000),
    ],
  ] as const)('closes the exit-during-listener race in %s', async (_name, operation) => {
    const child = fakeChild();
    const originalOnce = child.once.bind(child);
    child.once = ((event: string, listener: (...arguments_: unknown[]) => void) => {
      const result = originalOnce(event, listener);
      if (event === 'exit') child.exitCode = 70;
      return result;
    }) as typeof child.once;
    const started = Date.now();
    await operation(child);
    expect(Date.now() - started).toBeLessThan(100);
    expect(child.kill).not.toHaveBeenCalled();
    expect(child.listenerCount('exit')).toBe(0);
  });

  it.each(['complete', 'cleanup_pending'] as const)(
    'accepts authenticated terminal %s delivered after child exit and before EOF',
    async (terminal) => {
      const child = fakeChild();
      const status = new PassThrough();
      const supervised = superviseUninstallStatus(
        child as never,
        status,
        new PassThrough(),
        handoff,
        transaction,
        () => {
          child.exitCode = 70;
          child.emit('exit', 70, null);
          setTimeout(() => status.end(authenticatedStatus(terminal)), 0);
          return Promise.resolve();
        },
        20,
        100,
        20,
        20,
      );
      status.write(authenticatedStatus('uninstall_ready'));
      await expect(supervised).resolves.toBe(terminal);
    },
  );

  it.each(['complete', 'cleanup_pending'] as const)(
    'accepts buffered authenticated %s written before terminal wait despite recorded exit',
    async (terminal) => {
      const child = fakeChild();
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const supervised = superviseUninstallStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        () => {
          status.write(authenticatedStatus(terminal));
          child.exitCode = 0;
          return Promise.resolve();
        },
        20,
        40,
        20,
        20,
      );
      status.write(authenticatedStatus('uninstall_ready'));
      await expect(supervised).resolves.toBe(terminal);
      expect(status.listenerCount('data')).toBe(0);
      expect(status.listenerCount('error')).toBe(0);
      expect(child.listenerCount('exit')).toBe(0);
    },
  );

  it.each(['partial', 'forged'] as const)(
    'rejects child exit with buffered %s terminal status',
    async (fault) => {
      const child = fakeChild();
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const supervised = superviseUninstallStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        () => {
          status.end(
            fault === 'partial'
              ? '{"version":1'
              : `${JSON.stringify({ version: 1, transaction, state: 'complete', mac: '00'.repeat(32) })}\n`,
          );
          child.exitCode = 70;
          return Promise.resolve();
        },
        20,
        40,
        20,
        20,
      );
      status.write(authenticatedStatus('uninstall_ready'));
      await expect(supervised).rejects.toThrow(fault === 'forged' ? 'authenticated' : 'exited');
      expect(status.listenerCount('data')).toBe(0);
      expect(status.listenerCount('error')).toBe(0);
      expect(child.listenerCount('exit')).toBe(0);
    },
  );

  it.each(['complete', 'cleanup_pending'] as const)(
    'keeps uninstall supervision attached through observable terminal %s',
    async (terminal) => {
      const child = fakeChild();
      const status = new PassThrough();
      const cancellation = new PassThrough();
      const removeInstalledApp = vi.fn(() => {
        expect(cancellation.writableEnded).toBe(false);
        setTimeout(() => {
          status.write(authenticatedStatus(terminal));
          child.exitCode = 0;
          child.emit('exit', 0, null);
        }, 0);
        return Promise.resolve();
      });
      const supervised = superviseUninstallStatus(
        child as never,
        status,
        cancellation,
        handoff,
        transaction,
        removeInstalledApp,
        20,
        40,
        20,
        20,
      );
      status.write(authenticatedStatus('uninstall_ready'));
      await expect(supervised).resolves.toBe(terminal);
      expect(removeInstalledApp).toHaveBeenCalledOnce();
      expect(cancellation.writableEnded).toBe(true);
      expect(child.kill).not.toHaveBeenCalled();
    },
  );

  it('bounds terminal observation once after uninstall handoff', async () => {
    const child = fakeChild();
    const status = new PassThrough();
    const cancellation = new PassThrough();
    const removeInstalledApp = vi.fn(() => Promise.resolve());
    const supervised = superviseUninstallStatus(
      child as never,
      status,
      cancellation,
      handoff,
      transaction,
      removeInstalledApp,
      10,
      15,
      10,
      10,
    );
    status.write(authenticatedStatus('uninstall_ready'));
    await expect(supervised).rejects.toThrow('deadline');
    expect(removeInstalledApp).toHaveBeenCalledOnce();
    expect(cancellation.writableEnded).toBe(true);
    expect(child.kill).toHaveBeenCalledTimes(1);
  });

  it('never accepts generic ready for uninstall', async () => {
    const child = fakeChild();
    const status = new PassThrough();
    const cancellation = new PassThrough();
    const supervised = superviseUninstallStatus(
      child as never,
      status,
      cancellation,
      handoff,
      transaction,
      () => Promise.resolve(),
      10,
      15,
      10,
      10,
    );
    status.write(authenticatedStatus('ready'));
    await expect(supervised).rejects.toThrow('authenticated');
    expect(cancellation.writableEnded).toBe(true);
    expect(child.kill).toHaveBeenCalledTimes(1);
  });
});
