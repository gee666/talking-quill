import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  ConcurrentFileReplacementError,
  preserveInvalidFile,
  readUtf8File,
  writeJsonAtomic,
} from '../../app/src/main/persistence/atomic-json';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

async function testPath(): Promise<string> {
  const directory = await createTestDirectory('atomic-json');
  directories.push(directory);
  return join(directory, 'settings.json');
}

describe('atomic JSON invalid-data quarantine', () => {
  it('quarantines only the source supplied by the startup read', async () => {
    const path = await testPath();
    const invalidSource = '{broken-json';
    await writeFile(path, invalidSource, 'utf8');

    const destination = await preserveInvalidFile(path, invalidSource);

    expect(destination).not.toBeNull();
    await expect(readFile(path)).rejects.toMatchObject({ code: 'ENOENT' });
    const diagnostic = JSON.parse(await readFile(destination ?? '', 'utf8')) as {
      readonly byteLength: number;
      readonly sha256: string;
    };
    expect(diagnostic).toMatchObject({
      byteLength: Buffer.byteLength(invalidSource),
      sha256: createHash('sha256').update(invalidSource).digest('hex'),
    });
    expect(await readFile(destination ?? '', 'utf8')).not.toContain(invalidSource);
  });

  it('quarantines malformed UTF-8 using the same decoding as the startup read', async () => {
    const path = await testPath();
    await writeFile(path, Buffer.from([0xff, 0xfe, 0x7b]));
    const source = await readUtf8File(path);
    if (source === null) throw new Error('Fixture missing');

    await expect(preserveInvalidFile(path, source)).resolves.not.toBeNull();
    await expect(readFile(path)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('restores and reports a valid replacement installed after the startup read', async () => {
    const path = await testPath();
    const invalidSource = '{broken-json';
    const replacement = { schemaVersion: 1, enabled: true };
    await writeFile(path, invalidSource, 'utf8');
    await writeJsonAtomic(path, replacement);

    await expect(preserveInvalidFile(path, invalidSource)).rejects.toBeInstanceOf(
      ConcurrentFileReplacementError,
    );

    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(replacement);
    const entries = await readdir(join(path, '..'));
    expect(entries.some((entry) => entry.endsWith('.invalid'))).toBe(false);
    expect(entries.some((entry) => entry.endsWith('.invalid-source'))).toBe(false);
  });

  it('never removes a replacement published while the invalid source is being claimed', async () => {
    const path = await testPath();
    const invalidSource = '{broken-json';
    const replacement = { schemaVersion: 1, enabled: true };
    await writeFile(path, invalidSource, 'utf8');

    const preservation = preserveInvalidFile(path, invalidSource);
    const replacementWrite = writeJsonAtomic(path, replacement);
    const [preservationResult, replacementResult] = await Promise.allSettled([
      preservation,
      replacementWrite,
    ]);

    expect(replacementResult.status).toBe('fulfilled');
    if (preservationResult.status === 'rejected') {
      expect(preservationResult.reason).toBeInstanceOf(ConcurrentFileReplacementError);
    }
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(replacement);
  });
});
