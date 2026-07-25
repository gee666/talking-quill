import { z } from 'zod';
import { HelperReadinessSchema } from './helper-readiness';
import { MicrophonePermissionStateSchema } from './audio';

export const UpdateCheckResultSchema = z
  .object({
    status: z.enum(['current', 'available']),
    currentVersion: z.string().min(1).max(64),
    latestVersion: z.string().min(1).max(64),
    releaseUrl: z.url().max(2_048),
  })
  .strict();

export const InfoStatusSchema = z
  .object({
    microphone: MicrophonePermissionStateSchema,
    screenRecording: z.enum(['granted', 'denied', 'unknown']),
    helper: HelperReadinessSchema,
  })
  .strict();

export const InfoPermissionSchema = z.enum([
  'microphone',
  'accessibility',
  'input-monitoring',
  'screen-recording',
]);
export const InfoLocationSchema = z.enum(['data', 'logs']);
export const ThirdPartyNoticesSchema = z.string().min(1).max(2_000_000);

export type UpdateCheckResult = z.infer<typeof UpdateCheckResultSchema>;
export type InfoStatus = z.infer<typeof InfoStatusSchema>;
export type InfoPermission = z.infer<typeof InfoPermissionSchema>;
export type InfoLocation = z.infer<typeof InfoLocationSchema>;
