import { z } from 'zod';
import { HelperReadinessSchema } from './helper-readiness';

export const AppStatusSchema = z.enum([
  'disabled',
  'ready',
  'recording',
  'transcribing',
  'processing',
  'needs-setup',
]);
export const AppStateSchema = z
  .object({
    enabled: z.boolean(),
    status: AppStatusSchema,
    modelReady: z.boolean(),
    helper: HelperReadinessSchema,
  })
  .strict();

export type AppStatus = z.infer<typeof AppStatusSchema>;
export type AppState = z.infer<typeof AppStateSchema>;
