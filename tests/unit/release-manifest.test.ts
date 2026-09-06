import { describe, expect, it } from 'vitest';
import {
  canonicalJson,
  sealReleaseManifest,
  validateReleaseManifest,
} from '../../scripts/release-manifest.mjs';

const body = () => ({
  schemaVersion: 2,
  repository: 'gee666/talking-quill',
  tag: 'v0.0.69',
  version: '0.0.69',
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'd'.repeat(40),
  platform: 'win',
  architecture: 'x64',
  promotable: true,
  workflowRunId: null,
  generatedAt: null,
  provenance: [
    {
      name: 'provenance-win-x64-setup.json',
      platform: 'win',
      arch: 'x64',
      mode: 'setup',
      sourceTree: 'd'.repeat(40),
      sourceTreeSha256: 'b'.repeat(64),
    },
    {
      name: 'provenance-win-x64-update.json',
      platform: 'win',
      arch: 'x64',
      mode: 'update',
      sourceTree: 'd'.repeat(40),
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

  it('accepts fresh-only manifests but rejects incomplete or mixed architecture update sets', () => {
    const fresh = {
      ...body(),
      provenance: body().provenance.filter(({ mode }) => mode === 'setup'),
    };
    expect(validateReleaseManifest(sealReleaseManifest(fresh)).provenance).toHaveLength(1);
    expect(() => sealReleaseManifest({ ...fresh, architecture: 'x64+arm64' })).toThrow(
      /incomplete/u,
    );
    const armSetup = {
      ...fresh.provenance[0],
      name: 'provenance-win-arm64-setup.json',
      arch: 'arm64',
    };
    expect(() =>
      sealReleaseManifest({
        ...body(),
        architecture: 'x64+arm64',
        provenance: [armSetup, ...body().provenance],
      }),
    ).toThrow(/incomplete/u);
    expect(() =>
      sealReleaseManifest({
        ...body(),
        provenance: body().provenance.filter(({ mode }) => mode === 'update'),
      }),
    ).toThrow(/incomplete/u);
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
      'architecture provenance is incomplete',
    );
  });
});
