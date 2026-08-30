import { createRequire } from 'node:module';
import { describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
const markerPolicy = require('../../scripts/forbidden-production-markers.cjs') as {
  readonly FORBIDDEN_MARKER_OVERLAP_BYTES: number;
  readonly FORBIDDEN_PRODUCTION_MARKERS: Readonly<Record<string, string>>;
  readonly assertNoForbiddenProductionMarkers: (path: string, bytes: Buffer) => void;
  readonly findForbiddenProductionMarker: (bytes: Buffer) => { readonly id: string } | null;
};
const FORBIDDEN_MARKER_OVERLAP_BYTES = markerPolicy.FORBIDDEN_MARKER_OVERLAP_BYTES;
const FORBIDDEN_PRODUCTION_MARKERS = markerPolicy.FORBIDDEN_PRODUCTION_MARKERS;
const assertNoForbiddenProductionMarkers = (path: string, bytes: Buffer) =>
  markerPolicy.assertNoForbiddenProductionMarkers(path, bytes);
const findForbiddenProductionMarker = (bytes: Buffer) =>
  markerPolicy.findForbiddenProductionMarker(bytes);
const installedRequest = FORBIDDEN_PRODUCTION_MARKERS.installedRequest;
const packagedTestEnvironment = FORBIDDEN_PRODUCTION_MARKERS.packagedTestEnvironment;
if (installedRequest === undefined || packagedTestEnvironment === undefined) {
  throw new Error('Required production markers are missing');
}

describe('canonical artifact marker scan', () => {
  it.each(['utf8', 'utf16le'] as const)('detects every marker in %s bytes', (encoding) => {
    for (const [id, marker] of Object.entries(FORBIDDEN_PRODUCTION_MARKERS)) {
      expect(
        findForbiddenProductionMarker(Buffer.from(`prefix${marker}suffix`, encoding)),
      ).toMatchObject({
        id,
      });
    }
  });

  it('detects a marker that crosses streamed scan chunks', () => {
    const marker = Buffer.from(installedRequest, 'utf8');
    const split = 7;
    let overlap = Buffer.alloc(0);
    let found = null;
    for (const chunk of [marker.subarray(0, split), marker.subarray(split)]) {
      const combined = Buffer.concat([overlap, chunk]);
      found ??= findForbiddenProductionMarker(combined);
      overlap = combined.subarray(Math.max(0, combined.length - FORBIDDEN_MARKER_OVERLAP_BYTES));
    }
    expect(found).toMatchObject({ id: 'installedRequest' });
  });

  it('reports the physical package path and accepts ordinary binary bytes', () => {
    expect(() =>
      assertNoForbiddenProductionMarkers('helper.exe', Buffer.from([0, 1, 2])),
    ).not.toThrow();
    expect(() =>
      assertNoForbiddenProductionMarkers(
        'resources/app.asar',
        Buffer.from(packagedTestEnvironment),
      ),
    ).toThrow(/packagedTestEnvironment.*resources\/app\.asar/u);
  });
});
