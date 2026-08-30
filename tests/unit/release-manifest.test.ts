import { describe, expect, it } from 'vitest';
import {
  canonicalJson,
  sealReleaseManifest,
  validateReleaseManifest,
} from '../../scripts/release-manifest.mjs';

const body = () => ({
  schemaVersion: 1,
  repository: 'gee666/talking-quill',
  tag: 'v0.0.68',
  version: '0.0.68',
  sourceCommit: 'a'.repeat(40),
  platform: 'win',
  architecture: 'x64',
  promotable: true,
  workflowRunId: null,
  generatedAt: null,
  provenance: [
    {
      name: 'provenance-win-x64.json',
      platform: 'win',
      arch: 'x64',
      sourceTreeSha256: 'b'.repeat(64),
    },
  ],
  assets: [{ name: 'Talking-Quill.exe', bytes: 3, sha256: 'c'.repeat(64) }],
});

describe('sealed release manifest', () => {
  it('uses key-order-independent canonical JSON and authenticates every field', () => {
    expect(canonicalJson({ z: 1, a: { y: 2, x: 3 } })).toBe('{"a":{"x":3,"y":2},"z":1}');
    const sealed = sealReleaseManifest(body());
    expect(validateReleaseManifest(sealed)).toBe(sealed);
    expect(() => validateReleaseManifest({ ...sealed, workflowRunId: 'changed' })).toThrow(
      'canonical digest mismatch',
    );
  });

  it('rejects unknown fields, unsafe names, unsorted names, and incomplete combined provenance', () => {
    expect(() => sealReleaseManifest({ ...body(), freshInstall: true })).toThrow('fields');
    expect(() =>
      sealReleaseManifest({
        ...body(),
        assets: [
          { name: 'z', bytes: 1, sha256: 'c'.repeat(64) },
          { name: 'A', bytes: 1, sha256: 'd'.repeat(64) },
        ],
      }),
    ).toThrow('sorted and unique');
    expect(() => sealReleaseManifest({ ...body(), architecture: 'x64+arm64' })).toThrow(
      'architectures are incomplete',
    );
  });
});
