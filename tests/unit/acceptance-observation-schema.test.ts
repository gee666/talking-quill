import { describe, expect, it } from 'vitest';
import {
  EndpointObservabilitySchema,
  PauseLeaseRenewalSchema,
} from '../../app/src/main/acceptance/installed-observation';

const peer = {
  processId: 41,
  creationMarker: '133700000000000001',
  integrityRid: 8192,
  sessionId: 3,
  userSidHash: '11'.repeat(32),
} as const;
const owner = {
  starts: 1,
  cleanExits: 0,
  abnormalExits: 0,
  singletonCollisions: 0,
  authAttempts: 1,
  authFailures: { crossUser: 0, wrongSession: 0, codeIdentity: 0, mac: 0, protocol: 0 },
  leaseAcquired: 1,
  leaseRenewed: 7,
  leaseExpired: 2,
  leaseDisconnected: 0,
  leaseReleasedNeutral: 0,
  leaseReleasedDraining: 0,
  drainDurationMsTotal: 0,
  drainDurationMsMax: 0,
  maintenancePostponed: 0,
  handoffSucceeded: 0,
  handoffFailed: 0,
  degraded: 0,
  hookRecoveries: 0,
} as const;

describe('installed acceptance evidence schemas', () => {
  it('requires authenticated endpoint peers from one session and user', () => {
    const value = {
      endpointVersion: 2,
      peerAuthenticated: true,
      releaseBuildDigest: '22'.repeat(32),
      manifestSha256: '33'.repeat(32),
      gateway: peer,
      owner: { ...peer, processId: 42, creationMarker: '133700000000000002' },
    } as const;
    expect(EndpointObservabilitySchema.safeParse(value).success).toBe(true);
    expect(
      EndpointObservabilitySchema.safeParse({
        ...value,
        owner: { ...value.owner, sessionId: 4 },
      }).success,
    ).toBe(false);
    expect(
      EndpointObservabilitySchema.safeParse({
        ...value,
        owner: { ...value.owner, userSidHash: '44'.repeat(32) },
      }).success,
    ).toBe(false);
  });

  it('requires the fixed pause, one expiry, and no renewal', () => {
    const value = {
      pauseDurationMs: 6_500,
      beforeTimestampMs: 1_700_000_000_000,
      afterTimestampMs: 1_700_000_006_500,
      before: owner,
      after: { ...owner, leaseExpired: 3 },
    } as const;
    expect(PauseLeaseRenewalSchema.safeParse(value).success).toBe(true);
    expect(
      PauseLeaseRenewalSchema.safeParse({ ...value, afterTimestampMs: value.beforeTimestampMs + 1 })
        .success,
    ).toBe(false);
    expect(
      PauseLeaseRenewalSchema.safeParse({ ...value, after: { ...value.after, leaseExpired: 4 } })
        .success,
    ).toBe(false);
    expect(
      PauseLeaseRenewalSchema.safeParse({ ...value, after: { ...value.after, leaseRenewed: 8 } })
        .success,
    ).toBe(false);
    expect(
      PauseLeaseRenewalSchema.safeParse({ ...value, after: { ...value.after, leaseExpired: 3.5 } })
        .success,
    ).toBe(false);
    expect(
      PauseLeaseRenewalSchema.safeParse({ ...value, before: { ...value.before, starts: -1 } })
        .success,
    ).toBe(false);
  });
});
