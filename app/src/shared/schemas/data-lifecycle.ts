import { z } from 'zod';

export const RESET_CONFIRMATION = 'RESET TALKING QUILL' as const;

export const ResetApplicationDataRequestSchema = z
  .object({ confirmation: z.literal(RESET_CONFIRMATION) })
  .strict();

export const ResetAcknowledgementTokenSchema = z.uuid();

export const ResetApplicationDataResultSchema = z
  .object({
    accepted: z.literal(true),
    acknowledgementToken: ResetAcknowledgementTokenSchema,
  })
  .strict();

export type ResetApplicationDataResult = z.infer<typeof ResetApplicationDataResultSchema>;
