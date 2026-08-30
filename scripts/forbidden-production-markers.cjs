/* global module */
const FORBIDDEN_PRODUCTION_MARKERS = Object.freeze({
  installedRequest: '--talking-quill-acceptance-request=',
  installedReadiness: '--talking-quill-installed-readiness-pipe=',
  installedProfile: '--talking-quill-installed-lifecycle-user-data=',
  installedPhysical: '--talking-quill-installed-physical-observation',
  installedAutomation: '--talking-quill-installed-automation-validation',
  acceptanceEndpoint: 'acceptance.endpoint_observability',
  acceptanceLeasePause: 'acceptance.pause_lease_renewal',
  acceptanceManifestPurpose: 'talking-quill/installed-acceptance-build',
  acceptanceManifestFile: 'windows-installed-acceptance-v1.txt',
  acceptanceBroker: '--windows-installed-acceptance-broker-v1',
  acceptanceBuildEnvironment: 'TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD',
  packagedTestEnvironment: 'TALKING_QUILL_PACKAGED_TEST',
  packagedTestProfile: '--talking-quill-user-data=',
  generalizedApplicationDispatch: 'runExtension',
  generalizedHelperDispatch: 'requestExtension',
  acceptanceApplicationDispatch: 'runInstalledAcceptance',
  acceptanceHelperDispatch: 'requestAcceptance',
  acceptanceDiagnosticFailure: 'acceptance-injected-write-failure',
  installedReadinessFailure: 'Installed readiness test failed',
  installedPhysicalFailure: 'Installed physical observation did not traverse every boundary',
});

const encodedMarkers = Object.freeze(
  Object.entries(FORBIDDEN_PRODUCTION_MARKERS).flatMap(([id, marker]) => [
    Object.freeze({
      id,
      encoding: 'utf8',
      bytes: Uint8Array.from(marker, (character) => character.charCodeAt(0)),
    }),
    Object.freeze({ id, encoding: 'utf16le', bytes: encodeUtf16Le(marker) }),
  ]),
);
const FORBIDDEN_MARKER_OVERLAP_BYTES = Math.max(
  ...encodedMarkers.map(({ bytes }) => bytes.length - 1),
);

function encodeUtf16Le(value) {
  const bytes = new Uint8Array(value.length * 2);
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    bytes[index * 2] = code & 0xff;
    bytes[index * 2 + 1] = code >>> 8;
  }
  return bytes;
}

function findForbiddenProductionMarker(bytes) {
  for (const marker of encodedMarkers) {
    if (bytes.includes(marker.bytes)) return { id: marker.id, encoding: marker.encoding };
  }
  return null;
}

function assertNoForbiddenProductionMarkers(path, bytes) {
  const found = findForbiddenProductionMarker(bytes);
  if (found !== null) {
    throw new Error(
      `Canonical package contains forbidden ${found.id} marker (${found.encoding}): ${path}`,
    );
  }
}

module.exports = {
  FORBIDDEN_MARKER_OVERLAP_BYTES,
  FORBIDDEN_PRODUCTION_MARKERS,
  assertNoForbiddenProductionMarkers,
  findForbiddenProductionMarker,
};
