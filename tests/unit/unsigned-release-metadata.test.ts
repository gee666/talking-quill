import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { canonicalizeUpdateMetadata } from '../../scripts/stage-unsigned-release.mjs';

const bytes = new Map([
  ['Talking-Quill-1.2.3-mac-arm64.dmg', Buffer.from('dmg')],
  ['Talking-Quill-1.2.3-mac-arm64.zip', Buffer.from('zip')],
]);
const evidence = (name: string) => {
  const value = bytes.get(name);
  if (value === undefined) return Promise.reject(new Error(`Missing fixture: ${name}`));
  return Promise.resolve({
    size: value.length,
    sha512: createHash('sha512').update(value).digest('base64'),
  });
};

const entry = async (name: string) => ({ url: name, ...(await evidence(name)) });

describe('unsigned updater metadata staging', () => {
  it('accepts normal macOS DMG plus ZIP metadata and emits a ZIP-only updater channel', async () => {
    const dmg = await entry('Talking-Quill-1.2.3-mac-arm64.dmg');
    const zip = await entry('Talking-Quill-1.2.3-mac-arm64.zip');
    const result = await canonicalizeUpdateMetadata(
      {
        version: '1.2.3',
        files: [dmg, zip],
        path: zip.url,
        sha512: zip.sha512,
        releaseDate: '2026-08-01T00:00:00.000Z',
      },
      {
        expectedVersion: '1.2.3',
        allowedFiles: [dmg.url, zip.url],
        expectedUpdateFile: zip.url,
        evidence,
      },
    );
    expect(result).toEqual({
      version: '1.2.3',
      files: [zip],
      path: zip.url,
      sha512: zip.sha512,
      releaseDate: '2026-08-01T00:00:00.000Z',
    });
  });

  it('rejects updater paths and bytes outside the expected architecture payloads', async () => {
    const zip = await entry('Talking-Quill-1.2.3-mac-arm64.zip');
    await expect(
      canonicalizeUpdateMetadata(
        {
          version: '1.2.3',
          files: [{ ...zip, url: `nested/${zip.url}` }],
          path: zip.url,
          sha512: zip.sha512,
        },
        {
          expectedVersion: '1.2.3',
          allowedFiles: [zip.url],
          expectedUpdateFile: zip.url,
          evidence,
        },
      ),
    ).rejects.toThrow('unexpected path');
  });
});
