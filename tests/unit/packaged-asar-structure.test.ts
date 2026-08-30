import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createPackageWithOptions } from '@electron/asar';
import { afterEach, describe, expect, it } from 'vitest';
import { verifyTargetNativeOnnxArchitecture } from '../../scripts/packaged-asar-structure.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
const tuples = [
  ['win', 'win32', 'x64'],
  ['win', 'win32', 'arm64'],
  ['mac', 'darwin', 'x64'],
  ['mac', 'darwin', 'arm64'],
] as const;

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => removeTestDirectory(root)));
});

describe('post-pack target-native ONNX gate', () => {
  it.each(tuples)(
    'accepts only a matching %s/%s package image',
    async (platform, directory, architecture) => {
      const archive = await nativeFixture(directory, architecture, architecture);

      await expect(
        verifyTargetNativeOnnxArchitecture(archive, { platform, architecture }),
      ).resolves.toBeUndefined();
    },
  );

  it('rejects a target-path binary with the wrong native architecture', async () => {
    const archive = await nativeFixture('win32', 'arm64', 'x64');

    await expect(
      verifyTargetNativeOnnxArchitecture(archive, { platform: 'win', architecture: 'arm64' }),
    ).rejects.toThrow('Post-pack ONNX native mismatch');
  });

  it('rejects a target-native file packed inside the ASAR', async () => {
    const archive = await nativeFixture('win32', 'x64', 'x64', false);

    await expect(
      verifyTargetNativeOnnxArchitecture(archive, { platform: 'win', architecture: 'x64' }),
    ).rejects.toThrow('Post-pack ONNX native is not unpacked');
  });
});

async function nativeFixture(
  platform: 'win32' | 'darwin',
  targetArchitecture: 'x64' | 'arm64',
  imageArchitecture: 'x64' | 'arm64',
  unpack = true,
): Promise<string> {
  const root = await createTestDirectory(`post-pack-${platform}-${targetArchitecture}`);
  roots.push(root);
  const source = resolve(root, 'source');
  const entry = resolve(
    source,
    'node_modules',
    'onnxruntime-node',
    'bin',
    'napi-v3',
    platform,
    targetArchitecture,
    'onnxruntime_binding.node',
  );
  await mkdir(resolve(entry, '..'), { recursive: true });
  await writeFile(entry, platform === 'win32' ? pe(imageArchitecture) : mach(imageArchitecture));
  const archive = resolve(root, 'fixture.asar');
  await createPackageWithOptions(source, archive, unpack ? { unpack: '**/*.node' } : {});
  return archive;
}

function pe(architecture: 'x64' | 'arm64'): Buffer {
  const bytes = Buffer.alloc(256);
  bytes.writeUInt16LE(0x5a4d, 0);
  bytes.writeUInt32LE(0x80, 0x3c);
  bytes.writeUInt32LE(0x00004550, 0x80);
  bytes.writeUInt16LE(architecture === 'x64' ? 0x8664 : 0xaa64, 0x84);
  return bytes;
}

function mach(architecture: 'x64' | 'arm64'): Buffer {
  const bytes = Buffer.alloc(8);
  bytes.writeUInt32LE(0xfeedfacf, 0);
  bytes.writeUInt32LE(architecture === 'x64' ? 0x01000007 : 0x0100000c, 4);
  return bytes;
}
