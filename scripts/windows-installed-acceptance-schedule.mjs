export const MAX_ACCEPTANCE_RUN_MS = 80 * 60 * 1_000;

export const ACCEPTANCE_PHASE_SCHEDULE = Object.freeze([
  phase('upgrade', 2 * 60_000, 10 * 60_000),
  phase('artifact-layout-inspection', 10 * 60_000, 11 * 60_000),
  phase('legacy-cleanup', 11 * 60_000, 12 * 60_000),
  phase('persisted-profile-sentinel-normal-launch', 12 * 60_000, 16 * 60_000),
  phase('v2-endpoint-peer-checks', 16 * 60_000, 18 * 60_000),
  phase('heartbeat-readiness-120s', 18 * 60_000, 21 * 60_000),
  phase('neutral-gateway-crash-same-owner-reconnect', 22 * 60_000, 24 * 60_000),
  phase('lease-expiry', 24 * 60_000, 25 * 60_000),
  phase('electron-crash-relaunch', 26 * 60_000, 28 * 60_000),
  phase('normal-quit', 28 * 60_000, 29 * 60_000),
  phase('login-marker', 30 * 60_000, 31 * 60_000),
  phase('running-silent-repair', 32 * 60_000, 36 * 60_000),
  phase('injected-repair-failure-recovery', 36 * 60_000, 41 * 60_000),
  phase('uninstall-preserving-data', 41 * 60_000, 46 * 60_000),
  phase('reinstall', 46 * 60_000, 52 * 60_000),
  phase('diagnostics-disabled-failure', 53 * 60_000, 55 * 60_000),
  phase('manual-physical-observation', 57 * 60_000, 59 * 60_000),
  phase('supplemental-synthetic-observation', 60 * 60_000, 62 * 60_000),
  phase('residue', 62 * 60_000, 70 * 60_000),
]);

export const ACCEPTANCE_REQUEST_SCHEDULE = Object.freeze([
  request(
    'profile-normal-readiness',
    'persisted-profile-sentinel-normal-launch',
    'normal-readiness',
    15 * 60_000,
    16 * 60_000,
  ),
  request('endpoint-peer', 'v2-endpoint-peer-checks', 'endpoint-peer', 17 * 60_000, 18 * 60_000),
  request('heartbeat-120s', 'heartbeat-readiness-120s', 'heartbeat-120s', 18 * 60_000, 21 * 60_000),
  request(
    'gateway-reconnect-arm',
    'neutral-gateway-crash-same-owner-reconnect',
    'gateway-reconnect-arm',
    23 * 60_000,
    24 * 60_000,
  ),
  request(
    'lease-expiry-arm',
    'lease-expiry',
    'lease-expiry-arm',
    24 * 60_000 + 30_000,
    25 * 60_000,
  ),
  request(
    'electron-crash-arm',
    'electron-crash-relaunch',
    'electron-crash-arm',
    26 * 60_000,
    27 * 60_000,
  ),
  request(
    'electron-relaunch-readiness',
    'electron-crash-relaunch',
    'normal-readiness',
    27 * 60_000,
    28 * 60_000,
  ),
  request('login-marker', 'login-marker', 'login-marker', 30 * 60_000, 31 * 60_000),
  request(
    'repair-normal-readiness',
    'running-silent-repair',
    'normal-readiness',
    32 * 60_000,
    33 * 60_000,
  ),
  request('reinstall-normal-readiness', 'reinstall', 'normal-readiness', 50 * 60_000, 52 * 60_000),
  request(
    'diagnostics-disabled-failure',
    'diagnostics-disabled-failure',
    'diagnostics-disabled-failure',
    53 * 60_000,
    54 * 60_000,
  ),
  request(
    'manual-physical-observation',
    'manual-physical-observation',
    'manual-physical-observation',
    57 * 60_000,
    59 * 60_000,
  ),
  request(
    'supplemental-synthetic-observation',
    'supplemental-synthetic-observation',
    'supplemental-synthetic-observation',
    60 * 60_000,
    61 * 60_000,
  ),
]);

function phase(phaseName, latestStartOffsetMs, deadlineOffsetMs) {
  return Object.freeze({ phase: phaseName, latestStartOffsetMs, deadlineOffsetMs });
}

function request(invocationId, phaseName, command, latestStartOffsetMs, deadlineOffsetMs) {
  return Object.freeze({
    invocationId,
    phase: phaseName,
    command,
    latestStartOffsetMs,
    deadlineOffsetMs,
  });
}
