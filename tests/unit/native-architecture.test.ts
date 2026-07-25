import { mkdir, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  inspectNativeTree,
  parseNativeArchitecture,
  parseNativeArchitectures,
} from '../../scripts/native-architecture.mjs';

const fixtureRoot = resolve('tmp', 'native-architecture-fixtures');

function pe(machine: number): Buffer {
  const bytes = Buffer.alloc(128);
  bytes.writeUInt16LE(0x5a4d, 0);
  bytes.writeUInt32LE(64, 0x3c);
  bytes.writeUInt32LE(0x00004550, 64);
  bytes.writeUInt16LE(machine, 68);
  return bytes;
}

function thinMach(cpu: number, littleEndian: boolean): Buffer {
  const bytes = Buffer.alloc(8);
  if (littleEndian) {
    bytes.writeUInt32LE(0xfeedfacf, 0);
    bytes.writeUInt32LE(cpu, 4);
  } else {
    bytes.writeUInt32BE(0xfeedfacf, 0);
    bytes.writeUInt32BE(cpu, 4);
  }
  return bytes;
}

function fatMach(cpus: readonly number[], littleEndian: boolean, fat64 = false): Buffer {
  const entrySize = fat64 ? 32 : 20;
  const bytes = Buffer.alloc(8 + cpus.length * entrySize);
  const magic = fat64 ? 0xcafebabf : 0xcafebabe;
  if (littleEndian) {
    bytes.writeUInt32LE(magic, 0);
    bytes.writeUInt32LE(cpus.length, 4);
    cpus.forEach((cpu, index) => bytes.writeUInt32LE(cpu, 8 + index * entrySize));
  } else {
    bytes.writeUInt32BE(magic, 0);
    bytes.writeUInt32BE(cpus.length, 4);
    cpus.forEach((cpu, index) => bytes.writeUInt32BE(cpu, 8 + index * entrySize));
  }
  return bytes;
}

afterEach(async () => {
  await rm(fixtureRoot, { recursive: true, force: true });
});

describe('recursive native magic and architecture inspection', () => {
  it('recognizes x64 and arm64 PE binaries', () => {
    expect(parseNativeArchitecture(pe(0x8664), 'native.node', false)).toBe('x64');
    expect(parseNativeArchitecture(pe(0xaa64), 'native.node', false)).toBe('arm64');
    expect(parseNativeArchitectures(pe(0x8664), 'native.node')).toEqual({
      format: 'pe',
      architectures: ['x64'],
    });
  });

  it('recognizes little- and big-endian thin 64-bit Mach-O binaries', () => {
    for (const littleEndian of [true, false]) {
      expect(parseNativeArchitectures(thinMach(0x01000007, littleEndian), 'x64.dylib')).toEqual({
        format: 'mach-o-thin',
        architectures: ['x64'],
      });
      expect(parseNativeArchitectures(thinMach(0x0100000c, littleEndian), 'arm64.dylib')).toEqual({
        format: 'mach-o-thin',
        architectures: ['arm64'],
      });
    }
  });

  it('recognizes 32- and 64-bit fat headers in both byte orders', () => {
    for (const littleEndian of [true, false]) {
      for (const fat64 of [true, false]) {
        expect(
          parseNativeArchitectures(
            fatMach([0x01000007, 0x0100000c], littleEndian, fat64),
            'universal.dylib',
          ),
        ).toEqual({ format: 'mach-o-fat', architectures: ['arm64', 'x64'] });
      }
    }
  });

  it('recursively detects an extra wrong-architecture native fixture regardless of extension', async () => {
    await mkdir(resolve(fixtureRoot, 'nested'), { recursive: true });
    await Promise.all([
      writeFile(resolve(fixtureRoot, 'application.exe'), pe(0x8664)),
      writeFile(resolve(fixtureRoot, 'readme.bin'), Buffer.from('not native')),
      writeFile(resolve(fixtureRoot, 'nested', 'substituted.payload'), pe(0xaa64)),
    ]);
    await expect(inspectNativeTree(fixtureRoot, 'x64')).rejects.toThrow(
      'nested/substituted.payload is arm64, expected x64',
    );
  });

  it('allows only an exact reviewed auxiliary architecture exception', async () => {
    await mkdir(resolve(fixtureRoot, 'resources'), { recursive: true });
    await writeFile(resolve(fixtureRoot, 'resources', 'elevate.exe'), pe(0x014c));
    await expect(
      inspectNativeTree(fixtureRoot, 'x64', { 'resources/elevate.exe': 'x86' }),
    ).resolves.toEqual([
      {
        path: 'resources/elevate.exe',
        format: 'pe',
        architectures: ['x86'],
      },
    ]);
    await expect(
      inspectNativeTree(fixtureRoot, 'x64', { 'resources/other.exe': 'x86' }),
    ).rejects.toThrow('resources/elevate.exe is x86, expected x64');
  });

  it('accepts a universal Mach-O containing the target slice', async () => {
    await mkdir(fixtureRoot, { recursive: true });
    await writeFile(resolve(fixtureRoot, 'universal'), fatMach([0x01000007, 0x0100000c], false));
    await expect(inspectNativeTree(fixtureRoot, 'arm64')).resolves.toEqual([
      {
        path: 'universal',
        format: 'mach-o-fat',
        architectures: ['arm64', 'x64'],
      },
    ]);
  });

  it('rejects malformed and unsupported recognized native containers', () => {
    const invalidPe = pe(0x8664);
    invalidPe.writeUInt32LE(0xffff, 0x3c);
    expect(() => parseNativeArchitectures(invalidPe, 'bad.dll')).toThrow(
      'Invalid PE header offset',
    );
    expect(() => parseNativeArchitectures(thinMach(0x9999, true), 'bad.dylib')).toThrow(
      'Unsupported native executable architecture',
    );
    const mach32 = Buffer.alloc(8);
    mach32.writeUInt32LE(0xfeedface, 0);
    expect(() => parseNativeArchitectures(mach32, 'legacy.dylib')).toThrow(
      'Unsupported 32-bit Mach-O',
    );
    const truncatedFat = Buffer.alloc(8);
    truncatedFat.writeUInt32BE(0xcafebabe, 0);
    truncatedFat.writeUInt32BE(2, 4);
    expect(() => parseNativeArchitectures(truncatedFat, 'fat.dylib')).toThrow(
      'Invalid fat Mach-O architecture table',
    );
  });
});
