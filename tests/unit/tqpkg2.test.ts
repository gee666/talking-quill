import { createHash } from 'node:crypto';
import { zstdCompressSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import {
  canonicalJson,
  parseTqpkg2,
  tqpkg2TreeDigest,
  validateTqpkg2Path,
  zstdFrameLength,
} from '../../scripts/tqpkg2.mjs';

const sha = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
function fixture(
  mode: 'fresh' | 'update' | 'repair' | 'stale-schema2-cleanup' = 'fresh',
  mutate?: (manifest: Record<string, unknown>) => void,
) {
  const payloads = [
    ['resources/app.asar', Buffer.from('payload')],
    ['resources/helper/talking-quill-helper.exe', Buffer.from('gateway')],
    ['resources/helper/talking-quill-keyboard-owner.exe', Buffer.from('owner')],
    [
      'resources/helper/talking-quill-update-recovery-launcher.exe',
      Buffer.from('recovery-launcher'),
    ],
  ] as const;
  const blocks = payloads.map(([, content]) => zstdCompressSync(content));
  const files = payloads.map(([path, content], index) => ({
    path,
    mode: 0,
    size: content.length,
    sha256: sha(content),
    blockOffset: 0,
    blockSize: blocks[index]?.length ?? 0,
  }));
  const manifest: Record<string, unknown> = {
    architecture: 'x64',
    faultPhase: null,
    files,
    packageMode: mode,
    predecessor:
      mode === 'update'
        ? {
            version: '0.0.68',
            releaseBuildDigest: '1'.repeat(64),
            gatewaySha256: '2'.repeat(64),
            ownerSha256: '3'.repeat(64),
          }
        : null,
    schemaVersion: 2,
    sourceCommit: 'a'.repeat(40),
    sourceTree: 'b'.repeat(40),
    target: {
      releaseBuildDigest: '4'.repeat(64),
      gatewaySha256: sha(payloads[1][1]),
      ownerSha256: sha(payloads[2][1]),
      recoveryLauncherSha256: sha(payloads[3][1]),
    },
    treeSha256: tqpkg2TreeDigest(files),
    version: '0.0.69',
  };
  mutate?.(manifest);
  let manifestBytes = Buffer.from(canonicalJson(manifest));
  for (let index = 0; index < 4; index += 1) {
    let offset = manifestBytes.length;
    for (const file of files) {
      file.blockOffset = offset;
      offset += file.blockSize;
    }
    manifestBytes = Buffer.from(canonicalJson(manifest));
  }
  const stub = Buffer.alloc(512);
  stub.writeUInt16LE(0x5a4d, 0);
  stub.writeUInt32LE(64, 0x3c);
  stub.writeUInt32LE(0x4550, 64);
  stub.writeUInt16LE(0x8664, 68);
  stub.writeUInt16LE(0x20b, 88);
  stub.writeUInt16LE(2, 156);
  const packageBytes = Buffer.concat([manifestBytes, ...blocks]),
    footer = Buffer.alloc(128);
  footer.write('TQPKG2\0\0', 0, 'ascii');
  footer.writeUInt32LE(2, 8);
  footer.writeBigUInt64LE(BigInt(stub.length), 16);
  footer.writeBigUInt64LE(BigInt(packageBytes.length), 24);
  footer.writeBigUInt64LE(BigInt(manifestBytes.length), 32);
  Buffer.from(sha(packageBytes), 'hex').copy(footer, 40);
  Buffer.from(sha(manifestBytes), 'hex').copy(footer, 72);
  return Buffer.concat([stub, packageBytes, footer]);
}

describe('shared TQPKG2 parser', () => {
  it.each(['fresh', 'update', 'repair'] as const)('parses strict %s policy', (mode) => {
    expect(parseTqpkg2(fixture(mode), 'x64').manifest.packageMode).toBe(mode);
  });
  it('gates stale schema-2 cleanup packages to explicit inspection', () => {
    const bytes = fixture('stale-schema2-cleanup');
    expect(() => parseTqpkg2(bytes, 'x64')).toThrow();
    expect(parseTqpkg2(bytes, 'x64', { allowStaleSchema2Cleanup: true }).manifest.packageMode).toBe(
      'stale-schema2-cleanup',
    );
  });
  it.each(['../x', 'a\\b', 'a:b', 'CON', 'dir/nul.txt', 'a./b'])(
    'rejects hostile path %s',
    (path) => expect(() => validateTqpkg2Path(path)).toThrow(),
  );
  it('gates fault seams to explicit installed-acceptance inspection', () => {
    const bytes = fixture('repair', (manifest) => {
      manifest.faultPhase = 'published';
    });
    expect(() => parseTqpkg2(bytes, 'x64')).toThrow();
    expect(parseTqpkg2(bytes, 'x64', { allowAcceptanceFaults: true }).manifest.faultPhase).toBe(
      'published',
    );
  });
  it('rejects unknown schema fields and malformed predecessors', () => {
    expect(() =>
      parseTqpkg2(
        fixture('fresh', (manifest) => {
          manifest.unknown = true;
        }),
        'x64',
      ),
    ).toThrow();
    expect(() =>
      parseTqpkg2(
        fixture('fresh', (manifest) => {
          (manifest.target as Record<string, unknown>).unknown = true;
        }),
        'x64',
      ),
    ).toThrow();
    expect(() =>
      parseTqpkg2(
        fixture('update', (manifest) => {
          (manifest.predecessor as Record<string, unknown>).gatewaySha256 = 'bad';
        }),
        'x64',
      ),
    ).toThrow();
    expect(() =>
      parseTqpkg2(
        fixture('fresh', (manifest) => {
          (manifest.target as Record<string, unknown>).recoveryLauncherSha256 = '0'.repeat(64);
        }),
        'x64',
      ),
    ).toThrow('target native role');
  });
  it('rejects concatenated valid Zstd frames', () => {
    const first = zstdCompressSync(Buffer.from('first'));
    const second = zstdCompressSync(Buffer.from('second'));
    const concatenated = Buffer.concat([first, second]);
    expect(zstdFrameLength(concatenated)).toBe(first.length);
    expect(zstdFrameLength(concatenated)).not.toBe(concatenated.length);
  });
  it('rejects footer, package, reserved-byte, architecture, and trailing-frame mutations', () => {
    for (const mutate of [
      (bytes: Buffer) => {
        bytes[bytes.length - 1] = 1;
      },
      (bytes: Buffer) => {
        bytes[bytes.length - 128 + 104] = 1;
      },
      (bytes: Buffer) => {
        bytes[520] = bytes.readUInt8(520) ^ 1;
      },
      (bytes: Buffer) => {
        bytes[68] = 0;
      },
      (bytes: Buffer) => {
        bytes[bytes.length - 128 + 8] = 3;
      },
    ]) {
      const bytes = fixture();
      mutate(bytes);
      expect(() => parseTqpkg2(bytes, 'x64')).toThrow();
    }
  });
});
