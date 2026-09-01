import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';

import {
  GATEWAY_CANNOT_SUPPRESS_MARKER,
  MACOS_SERVICE_BRIDGE_MARKER,
  OWNER_LOCAL_ENABLED_MARKER,
  OWNER_SAFE_DISABLED_MARKER,
  SOURCE_COMMIT_MARKER,
  SOURCE_TREE_MARKER,
  WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
  WINDOWS_UPDATE_RECOVERY_LAUNCHER_MARKER,
  nativeRoleLayout,
  verifyCompleteNativeRoleInventory,
  verifyNativeSourceIdentity,
  verifyStagedNativeRoleSet,
} from '../../scripts/helper-build-contract.mjs';
import { replaceNativeRoleDirectory } from '../../scripts/native-staging.mjs';

const root = resolve('tmp', 'helper-role-staging-test');

function pe(architecture: 'x64' | 'arm64', marker: Buffer): Buffer {
  const header = Buffer.alloc(256);
  header.writeUInt16LE(0x5a4d, 0);
  header.writeUInt32LE(0x80, 0x3c);
  header.writeUInt32LE(0x0000_4550, 0x80);
  header.writeUInt16LE(architecture === 'x64' ? 0x8664 : 0xaa64, 0x84);
  return Buffer.concat([header, marker]);
}

function mach(architecture: 'x64' | 'arm64', marker: Buffer): Buffer {
  const header = Buffer.alloc(32);
  header.writeUInt32LE(0xfeed_facf, 0);
  header.writeUInt32LE(architecture === 'x64' ? 0x0100_0007 : 0x0100_000c, 4);
  return Buffer.concat([header, marker]);
}

async function stageWindows(architecture: 'x64' | 'arm64', directory = root): Promise<void> {
  await mkdir(directory, { recursive: true });
  await Promise.all([
    writeFile(
      resolve(directory, 'talking-quill-helper.exe'),
      pe(
        architecture,
        Buffer.concat([
          GATEWAY_CANNOT_SUPPRESS_MARKER,
          WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
          Buffer.from(`04${'11'.repeat(64)}`, 'ascii'),
        ]),
      ),
    ),
    writeFile(
      resolve(directory, 'talking-quill-keyboard-owner.exe'),
      pe(architecture, OWNER_LOCAL_ENABLED_MARKER),
    ),
    writeFile(
      resolve(directory, 'talking-quill-update-recovery-launcher.exe'),
      pe(architecture, WINDOWS_UPDATE_RECOVERY_LAUNCHER_MARKER),
    ),
  ]);
}

async function stageMac(architecture: 'x64' | 'arm64'): Promise<void> {
  await mkdir(root, { recursive: true });
  await Promise.all([
    writeFile(
      resolve(root, 'talking-quill-helper'),
      mach(architecture, GATEWAY_CANNOT_SUPPRESS_MARKER),
    ),
    writeFile(
      resolve(root, 'talking-quill-keyboard-owner'),
      mach(architecture, OWNER_LOCAL_ENABLED_MARKER),
    ),
    writeFile(
      resolve(root, 'talking-quill-macos-service-bridge'),
      mach(architecture, MACOS_SERVICE_BRIDGE_MARKER),
    ),
  ]);
}

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
});

describe('R9 role-separated native staging contract', () => {
  it('recovers a verified backup after process death and completes the next atomic swap', async () => {
    const appDirectory = resolve(root, 'app');
    const destination = resolve(appDirectory, 'native');
    const backup = resolve(appDirectory, '.native-backup-41');
    const staging = resolve(root, 'replacement');
    await stageWindows('x64', destination);
    await stageWindows('x64', staging);
    await rename(destination, backup);
    await mkdir(resolve(appDirectory, '.native-build-lock'));
    await writeFile(
      resolve(appDirectory, '.native-build-lock/owner.json'),
      `${JSON.stringify({ version: 1, pid: 41 })}\n`,
    );
    await replaceNativeRoleDirectory({
      appDirectory,
      stagingDirectory: staging,
      platform: 'win32',
      architecture: 'x64',
      pid: 42,
      processAlive: () => false,
    });
    await expect(
      verifyStagedNativeRoleSet(destination, { platform: 'win32', architecture: 'x64' }),
    ).resolves.toBeUndefined();
    await expect(
      readFile(resolve(appDirectory, '.native-build-lock/owner.json')),
    ).rejects.toThrow();
  });

  it.each([
    ['x64', 'arm64'],
    ['arm64', 'x64'],
  ] as const)('replaces a coherent %s app/native tree with %s roles', async (from, to) => {
    const appDirectory = resolve(root, `architecture-swap-${from}-${to}`);
    const destination = resolve(appDirectory, 'native');
    const staging = resolve(root, `architecture-replacement-${to}`);
    await stageWindows(from, destination);
    await stageWindows(to, staging);

    await replaceNativeRoleDirectory({
      appDirectory,
      stagingDirectory: staging,
      platform: 'win32',
      architecture: to,
      pid: 45,
      processAlive: () => false,
    });

    await expect(
      verifyStagedNativeRoleSet(destination, { platform: 'win32', architecture: to }),
    ).resolves.toBeUndefined();
  });

  it('rejects unexpected existing app/native content without deleting it', async () => {
    const appDirectory = resolve(root, 'dirty-app');
    const destination = resolve(appDirectory, 'native');
    const staging = resolve(root, 'dirty-replacement');
    await stageWindows('x64', destination);
    await writeFile(resolve(destination, 'developer-note.txt'), 'keep me');
    await stageWindows('x64', staging);
    await expect(
      replaceNativeRoleDirectory({
        appDirectory,
        stagingDirectory: staging,
        platform: 'win32',
        architecture: 'x64',
        pid: 44,
        processAlive: () => false,
      }),
    ).rejects.toThrow('role set mismatch');
    await expect(readFile(resolve(destination, 'developer-note.txt'), 'utf8')).resolves.toBe(
      'keep me',
    );
  });

  it('allows only one contender to reclaim the same stale lock generation', async () => {
    const appDirectory = resolve(root, 'race-app');
    const destination = resolve(appDirectory, 'native');
    const first = resolve(root, 'race-first');
    const second = resolve(root, 'race-second');
    await stageWindows('x64', destination);
    await stageWindows('x64', first);
    await stageWindows('x64', second);
    await mkdir(resolve(appDirectory, '.native-build-lock'));
    await writeFile(
      resolve(appDirectory, '.native-build-lock/owner.json'),
      `${JSON.stringify({ version: 1, pid: 40, token: 'stale' })}\n`,
    );
    const outcomes = await Promise.allSettled([
      replaceNativeRoleDirectory({
        appDirectory,
        stagingDirectory: first,
        platform: 'win32',
        architecture: 'x64',
        pid: 42,
        processAlive: () => false,
      }),
      replaceNativeRoleDirectory({
        appDirectory,
        stagingDirectory: second,
        platform: 'win32',
        architecture: 'x64',
        pid: 43,
        processAlive: () => false,
      }),
    ]);
    expect(outcomes.filter((outcome) => outcome.status === 'fulfilled')).toHaveLength(1);
    expect(outcomes.filter((outcome) => outcome.status === 'rejected')).toHaveLength(1);
    await expect(
      verifyStagedNativeRoleSet(destination, { platform: 'win32', architecture: 'x64' }),
    ).resolves.toBeUndefined();
  });

  it.each(['x64', 'arm64'] as const)('accepts the complete Windows %s role set', async (arch) => {
    await stageWindows(arch);
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: arch }),
    ).resolves.toBeUndefined();
  });

  it.each(['x64', 'arm64'] as const)('accepts the complete macOS %s role set', async (arch) => {
    await stageMac(arch);
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'darwin', architecture: arch }),
    ).resolves.toBeUndefined();
  });

  it('rejects native bytes built from another source commit or tree', async () => {
    const path = resolve(root, 'source-bound.exe');
    const identity = { sourceCommit: 'a'.repeat(40), sourceTree: 'b'.repeat(40) };
    await mkdir(root, { recursive: true });
    await writeFile(
      path,
      Buffer.concat([
        SOURCE_COMMIT_MARKER,
        Buffer.from(identity.sourceCommit),
        SOURCE_TREE_MARKER,
        Buffer.from(identity.sourceTree),
      ]),
    );
    await expect(verifyNativeSourceIdentity(path, identity)).resolves.toBeUndefined();
    await expect(
      verifyNativeSourceIdentity(path, { ...identity, sourceTree: 'c'.repeat(40) }),
    ).rejects.toThrow('source tree does not match');
    await writeFile(
      path,
      Buffer.concat([
        Buffer.alloc(SOURCE_COMMIT_MARKER.length - 1),
        Buffer.from(identity.sourceCommit),
        SOURCE_TREE_MARKER,
        Buffer.from(identity.sourceTree),
      ]),
    );
    await expect(verifyNativeSourceIdentity(path, identity)).rejects.toThrow(
      'source commit does not match',
    );
  });

  it('requires every assigned package role to be in the recognized native inventory', async () => {
    await stageWindows('x64');
    const gateway = resolve(root, 'talking-quill-helper.exe');
    const owner = resolve(root, 'talking-quill-keyboard-owner.exe');
    await expect(verifyCompleteNativeRoleInventory([gateway], [gateway, owner])).rejects.toThrow(
      'Assigned role is not a recognized native executable',
    );
  });

  it('defines the exact three-role Windows native inventory with one suppression owner', () => {
    expect(nativeRoleLayout('win32')).toEqual([
      expect.objectContaining({ role: 'gateway', suppressionCapable: false }),
      expect.objectContaining({ role: 'owner', suppressionCapable: true }),
      expect.objectContaining({ role: 'recovery-launcher', suppressionCapable: false }),
    ]);
    for (const platform of ['win32', 'darwin'] as const) {
      const layout = nativeRoleLayout(platform);
      expect(layout.filter((role) => role.suppressionCapable)).toEqual([
        expect.objectContaining({ role: 'owner' }),
      ]);
      expect(layout.find((role) => role.role === 'gateway')?.suppressionCapable).toBe(false);
      expect(
        layout
          .filter((role) => role.role === 'authority')
          .every((role) => !role.suppressionCapable),
      ).toBe(true);
    }
  });

  it('rejects mixed role and safe-disabled owner markers', async () => {
    await stageWindows('x64');
    await writeFile(
      resolve(root, 'talking-quill-helper.exe'),
      pe('x64', Buffer.concat([GATEWAY_CANNOT_SUPPRESS_MARKER, OWNER_LOCAL_ENABLED_MARKER])),
    );
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('suppression authority mismatch');

    await stageWindows('x64');
    await writeFile(
      resolve(root, 'talking-quill-keyboard-owner.exe'),
      pe('x64', Buffer.concat([OWNER_LOCAL_ENABLED_MARKER, OWNER_SAFE_DISABLED_MARKER])),
    );
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('forbidden gateway/safe/test marker');
  });

  it('rejects missing, extra, wrong-architecture, and second-owner bytes', async () => {
    await stageWindows('x64');
    await rm(resolve(root, 'talking-quill-update-recovery-launcher.exe'));
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('role set mismatch');

    await stageWindows('x64');
    await writeFile(
      resolve(root, 'talking-quill-update-recovery-launcher.exe'),
      pe('arm64', WINDOWS_UPDATE_RECOVERY_LAUNCHER_MARKER),
    );
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('architecture mismatch');

    await stageWindows('x64');
    await rm(resolve(root, 'talking-quill-keyboard-owner.exe'));
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('role set mismatch');

    await stageWindows('x64');
    await writeFile(resolve(root, 'junk.exe'), pe('x64', Buffer.from('junk')));
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('role set mismatch');

    await rm(root, { recursive: true, force: true });
    await stageWindows('arm64');
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'x64' }),
    ).rejects.toThrow('architecture mismatch');

    await writeFile(
      resolve(root, 'talking-quill-helper.exe'),
      pe('arm64', Buffer.concat([GATEWAY_CANNOT_SUPPRESS_MARKER, OWNER_LOCAL_ENABLED_MARKER])),
    );
    await expect(
      verifyStagedNativeRoleSet(root, { platform: 'win32', architecture: 'arm64' }),
    ).rejects.toThrow('suppression authority mismatch');
  });
});
