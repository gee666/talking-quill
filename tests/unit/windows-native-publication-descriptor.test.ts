import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  cleanupAcceptanceNativeDescriptor,
  validateDescriptor,
} from '../../scripts/windows-installed-acceptance-native-publication.mjs';

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(
    directories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

function descriptor(output: string) {
  const nativeBase = resolve(output, 'program-data', 'Talking Quill Acceptance Native');
  const publicationId = 'b'.repeat(64);
  const nativeRoot = resolve(nativeBase, publicationId);
  const inventory = [
    'talking-quill-acceptance-signer.exe',
    'talking-quill-windows-acceptance-broker.exe',
    'talking-quill-helper.exe',
  ].map((name) => ({
    name,
    path: resolve(nativeRoot, name),
    bytes: 1,
    sha256: 'c'.repeat(64),
    identity: '1:2',
  }));
  return {
    schemaVersion: 1,
    purpose: 'talking-quill/installed-acceptance-native-publication/v1',
    buildId: 'a'.repeat(64),
    publicationId,
    layout: 'producer',
    removeNativeBase: true,
    nativeBase,
    nativeBaseIdentity: '1:3',
    nativeRoot,
    userSid: 'S-1-5-21-123',
    rootIdentity: '1:4',
    inventory,
    cleanupLauncher: {
      path: resolve(output, 'native-publication-cleanup-helper.exe'),
      bytes: 1,
      sha256: 'c'.repeat(64),
      identity: '1:5',
    },
    descriptorPath: resolve(output, 'native-publication-cleanup.json'),
    descriptorSha256: 'd'.repeat(64),
  };
}

describe('native publication v1 descriptor contract', () => {
  it('accepts the declared v1 shape and rejects incomplete v2 receipts', () => {
    const value = descriptor(resolve('tmp', 'publication-contract'));
    expect(validateDescriptor(value)).toBe(value);
    expect(() => validateDescriptor({ ...value, schemaVersion: 2 })).toThrow(
      'descriptor is invalid',
    );
    expect(() =>
      validateDescriptor({ ...value, purpose: value.purpose.replace('/v1', '/v2') }),
    ).toThrow('descriptor is invalid');
    expect(() => validateDescriptor({ ...value, descriptorSha256: undefined })).toThrow(
      'descriptor is invalid',
    );
  });

  it('loads the descriptor and requires the caller-supplied digest before cleanup', async () => {
    await mkdir(resolve('tmp'), { recursive: true });
    const output = await mkdtemp(resolve('tmp', 'publication-contract-'));
    directories.push(output);
    const value = descriptor(output);
    await writeFile(value.descriptorPath, JSON.stringify(value));
    await expect(
      cleanupAcceptanceNativeDescriptor(value.descriptorPath, {
        descriptorSha256: '',
      }),
    ).rejects.toThrow('descriptor is invalid');
    // A valid caller digest reaches base authorization, without invoking any native tool.
    await expect(
      cleanupAcceptanceNativeDescriptor(value.descriptorPath, {
        descriptorSha256: value.descriptorSha256,
        programData: resolve(output, 'unauthorized'),
      }),
    ).rejects.toThrow('cleanup base is unauthorized');
  });
});
