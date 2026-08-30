import { createReadStream } from 'node:fs';
import { lstat, mkdir, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { Readable } from 'node:stream';
import { resolve } from 'node:path';
import { createPackageFromStreams, createPackageWithOptions, listPackage } from '@electron/asar';
import { afterEach, describe, expect, it } from 'vitest';
import { extractRegularAsarFiles } from '../../scripts/asar-entry-inspection.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const require = createRequire(import.meta.url);
const markerPolicy = require('../../scripts/forbidden-production-markers.cjs') as {
  FORBIDDEN_PRODUCTION_MARKERS: { packagedTestEnvironment: string };
  assertNoForbiddenProductionMarkers(path: string, bytes: Uint8Array): void;
};
const roots: string[] = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => removeTestDirectory(root)));
});

describe('ASAR regular-file inspection', () => {
  it('skips directories and scans packed text plus unpacked binary files', async () => {
    const root = await fixtureRoot('asar-regular');
    const source = resolve(root, 'source');
    const archive = resolve(root, 'fixture.asar');
    const marker = markerPolicy.FORBIDDEN_PRODUCTION_MARKERS.packagedTestEnvironment;
    await mkdir(resolve(source, 'nested'), { recursive: true });
    await writeFile(resolve(source, 'nested', 'safe.txt'), 'ordinary application text');
    await writeFile(resolve(source, 'payload.bin'), Buffer.from([0, 1, ...Buffer.from(marker), 2]));
    await createPackageWithOptions(source, archive, { unpack: '**/*.bin' });

    const files = [...extractRegularAsarFiles(archive, normalizedEntries(archive))];
    expect(files.map(({ entry }) => entry)).toEqual(['nested/safe.txt', 'payload.bin']);
    expect(files.find(({ entry }) => entry === 'payload.bin')?.metadata.unpacked).toBe(true);
    expect(() => {
      for (const file of files) {
        markerPolicy.assertNoForbiddenProductionMarkers(`app.asar/${file.entry}`, file.bytes);
      }
    }).toThrow(/forbidden packagedTestEnvironment marker.*payload\.bin/u);
  });

  it('rejects links before trying to extract them', async () => {
    const root = await fixtureRoot('asar-link');
    const archive = resolve(root, 'fixture.asar');
    const target = resolve(root, 'target.txt');
    await writeFile(target, 'target');
    const stat = await lstat(target);
    await createPackageFromStreams(archive, [
      {
        type: 'file',
        path: 'target.txt',
        unpacked: false,
        stat,
        streamGenerator: () => createReadStream(target),
      },
      {
        type: 'link',
        path: 'alias.txt',
        symlink: 'target.txt',
        unpacked: false,
        stat,
        streamGenerator: () => Readable.from([]),
      },
    ]);

    expect(() => [...extractRegularAsarFiles(archive, normalizedEntries(archive))]).toThrow(
      'ASAR link is not allowed: alias.txt',
    );
  });

  it('rejects malformed entry metadata without extraction', async () => {
    const root = await fixtureRoot('asar-malformed');
    const archive = resolve(root, 'malformed.asar');
    await writeAsar(archive, { offset: '0', bogus: 4 }, Buffer.alloc(4));

    expect(normalizedEntries(archive)).toEqual(['broken.bin']);
    expect(() => [...extractRegularAsarFiles(archive, normalizedEntries(archive))]).toThrow(
      'ASAR entry is malformed: broken.bin',
    );
  });

  it('rejects regular-file metadata that extends beyond the archive', async () => {
    const root = await fixtureRoot('asar-truncated');
    const archive = resolve(root, 'truncated.asar');
    await writeAsar(archive, { offset: '2', size: 8 }, Buffer.alloc(3));

    expect(() => [...extractRegularAsarFiles(archive, normalizedEntries(archive))]).toThrow(
      'ASAR regular file extends beyond the archive: broken.bin',
    );
  });

  it('requires a physical regular file for every unpacked entry, including empty files', async () => {
    const root = await fixtureRoot('asar-missing-unpacked');
    const archive = resolve(root, 'missing-unpacked.asar');
    await writeAsar(archive, { size: 0, unpacked: true }, Buffer.alloc(0));

    expect(() => [...extractRegularAsarFiles(archive, normalizedEntries(archive))]).toThrow(
      'ASAR unpacked regular file is missing: broken.bin',
    );
  });

  it('rejects malformed non-boolean unpacked metadata', async () => {
    const root = await fixtureRoot('asar-invalid-unpacked');
    const archive = resolve(root, 'invalid-unpacked.asar');
    await writeAsar(archive, { offset: '0', size: 0, unpacked: 'true' }, Buffer.alloc(0));

    expect(() => [...extractRegularAsarFiles(archive, normalizedEntries(archive))]).toThrow(
      'ASAR entry is malformed: broken.bin',
    );
  });
});

async function fixtureRoot(prefix: string): Promise<string> {
  const root = await createTestDirectory(prefix);
  roots.push(root);
  return root;
}

function normalizedEntries(archive: string): string[] {
  return listPackage(archive, { isPack: false }).map((entry) =>
    entry.replaceAll('\\', '/').replace(/^\/+/, ''),
  );
}

async function writeAsar(
  path: string,
  metadata: Record<string, unknown>,
  payload: Buffer,
): Promise<void> {
  const header = JSON.stringify({ files: { 'broken.bin': metadata } });
  const headerBytes = Buffer.from(header);
  const stringPadding = (4 - (headerBytes.length % 4)) % 4;
  const headerPickle = Buffer.alloc(8 + headerBytes.length + stringPadding);
  headerPickle.writeUInt32LE(4 + headerBytes.length + stringPadding, 0);
  headerPickle.writeUInt32LE(headerBytes.length, 4);
  headerBytes.copy(headerPickle, 8);
  const sizePickle = Buffer.alloc(8);
  sizePickle.writeUInt32LE(4, 0);
  sizePickle.writeUInt32LE(headerPickle.length, 4);
  await writeFile(path, Buffer.concat([sizePickle, headerPickle, payload]));
}
