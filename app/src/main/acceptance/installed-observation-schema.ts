import { z } from 'zod';
import { HelperOwnerObservabilitySchema } from '../../shared/helper/protocol';
import type { InstalledAcceptanceHelper } from './installed-observation-types';

const EndpointPeerSchema = z
  .object({
    processId: z.number().int().positive().max(0xffff_ffff),
    creationMarker: z.string().regex(/^[1-9][0-9]{0,19}$/u),
    integrityRid: z.number().int().min(0).max(0xffff_ffff),
    sessionId: z.number().int().min(0).max(0xffff_ffff),
    userSidHash: z.string().regex(/^[0-9a-f]{64}$/u),
  })
  .strict();
export const EndpointObservabilitySchema = z
  .object({
    endpointVersion: z.literal(2),
    peerAuthenticated: z.literal(true),
    releaseBuildDigest: z.string().regex(/^[0-9a-f]{64}$/u),
    manifestSha256: z.string().regex(/^[0-9a-f]{64}$/u),
    gateway: EndpointPeerSchema,
    owner: EndpointPeerSchema,
  })
  .strict()
  .superRefine((value, context) => {
    if (value.gateway.sessionId !== value.owner.sessionId) {
      context.addIssue({
        code: 'custom',
        path: ['owner', 'sessionId'],
        message: 'Authenticated endpoint peers must share a Windows session',
      });
    }
    if (value.gateway.userSidHash !== value.owner.userSidHash) {
      context.addIssue({
        code: 'custom',
        path: ['owner', 'userSidHash'],
        message: 'Authenticated endpoint peers must share a redacted user identity',
      });
    }
  });
export const PauseLeaseRenewalSchema = z
  .object({
    pauseDurationMs: z.literal(6_500),
    beforeTimestampMs: z.number().int().nonnegative(),
    afterTimestampMs: z.number().int().nonnegative(),
    before: HelperOwnerObservabilitySchema,
    after: HelperOwnerObservabilitySchema,
  })
  .strict()
  .superRefine((value, context) => {
    if (value.afterTimestampMs - value.beforeTimestampMs < value.pauseDurationMs) {
      context.addIssue({
        code: 'custom',
        path: ['afterTimestampMs'],
        message: 'Lease-renewal pause did not span the fixed duration',
      });
    }
    if (value.after.leaseExpired !== value.before.leaseExpired + 1) {
      context.addIssue({
        code: 'custom',
        path: ['after', 'leaseExpired'],
        message: 'Lease-renewal pause must prove exactly one expiry',
      });
    }
    if (value.after.leaseRenewed !== value.before.leaseRenewed) {
      context.addIssue({
        code: 'custom',
        path: ['after', 'leaseRenewed'],
        message: 'Lease renewal changed during the pause',
      });
    }
  });

export async function endpointObservability(helper: InstalledAcceptanceHelper) {
  return EndpointObservabilitySchema.parse(
    await helper.requestAcceptance(
      'acceptance.endpoint_observability',
      EndpointObservabilitySchema,
      3_000,
    ),
  );
}

export async function pauseLeaseRenewal(helper: InstalledAcceptanceHelper) {
  return PauseLeaseRenewalSchema.parse(
    await helper.requestAcceptance(
      'acceptance.pause_lease_renewal',
      PauseLeaseRenewalSchema,
      10_000,
    ),
  );
}
