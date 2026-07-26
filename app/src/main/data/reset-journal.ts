import { z } from 'zod';

export const RESET_JOURNAL_VERSION = 4 as const;
export const APP_OWNERSHIP_ID = 'com.talkingquill.app' as const;

const journalBase = {
  appId: z.literal(APP_OWNERSHIP_ID),
  userDataRoot: z.string().min(1).max(32_768),
  rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
  requestedAt: z.number().int().nonnegative(),
  nonce: z.uuid(),
};

const LegacyV2ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(2),
    ...journalBase,
  })
  .strict();

const LegacyV3ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(3),
    ...journalBase,
    rootFileIdentity: z.string().regex(/^\d+:\d+$/),
    tombstonePath: z.string().min(1).max(32_768),
  })
  .strict();

export const ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(RESET_JOURNAL_VERSION),
    ...journalBase,
    rootFileIdentity: z.string().regex(/^\d+:\d+$/),
    tombstonePath: z.string().min(1).max(32_768),
    disposalPath: z.string().min(1).max(32_768),
    phase: z.enum(['rename-pending', 'disposal-pending']),
  })
  .strict();

export type ResetJournal = z.infer<typeof ResetJournalSchema>;
export type DecodedResetJournal =
  | { readonly kind: 'legacy-v2'; readonly value: z.infer<typeof LegacyV2ResetJournalSchema> }
  | { readonly kind: 'legacy-v3'; readonly value: z.infer<typeof LegacyV3ResetJournalSchema> }
  | { readonly kind: 'current'; readonly value: ResetJournal };

/** Pure decoding keeps legacy interpretation separate from filesystem recovery side effects. */
export function decodeResetJournal(value: unknown): DecodedResetJournal {
  const legacyV2 = LegacyV2ResetJournalSchema.safeParse(value);
  if (legacyV2.success) return { kind: 'legacy-v2', value: legacyV2.data };
  const legacyV3 = LegacyV3ResetJournalSchema.safeParse(value);
  if (legacyV3.success) return { kind: 'legacy-v3', value: legacyV3.data };
  return { kind: 'current', value: ResetJournalSchema.parse(value) };
}
