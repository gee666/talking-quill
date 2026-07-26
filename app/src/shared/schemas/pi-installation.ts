import { z } from 'zod';
import { PublicProviderErrorCodeSchema } from './providers';
import { PiInstallationPathSchema } from './settings';

export const PiDiscoverySourceSchema = z.enum([
  'configured',
  'where',
  'path',
  'appdata-npm',
  'pnpm-home',
  'localappdata-pnpm',
]);

export const PiInstallationStatusSchema = z
  .object({
    mode: z.enum(['automatic', 'configured']),
    state: z.enum(['ready', 'not-found', 'invalid', 'incompatible']),
    configuredPath: PiInstallationPathSchema,
    path: z.string().min(1).max(8_192).nullable(),
    version: z.string().min(1).max(64).nullable(),
    source: PiDiscoverySourceSchema.nullable(),
    errorCode: PublicProviderErrorCodeSchema.nullable(),
  })
  .strict();
export type PiInstallationStatus = z.infer<typeof PiInstallationStatusSchema>;

export const PiInstallationSaveRequestSchema = z
  .object({ path: PiInstallationPathSchema })
  .strict();

export const PiInstallationBrowseResultSchema = z
  .object({ path: PiInstallationPathSchema })
  .strict();
export type PiInstallationBrowseResult = z.infer<typeof PiInstallationBrowseResultSchema>;
