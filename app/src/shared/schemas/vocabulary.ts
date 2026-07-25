import { z } from 'zod';
import { utf8ByteLength } from './text-bounds';

export const VOCABULARY_LIMIT = 1_000;
export const VOCABULARY_VALUE_MAX_LENGTH = 200;
export const VOCABULARY_VALUE_MAX_UTF8_BYTES = 400;
export const VOCABULARY_TOTAL_MAX_UTF8_BYTES = 256_000;
export const VOCABULARY_FILE_MAX_BYTES = 1_048_576;

export const VocabularyIdSchema = z.uuid();
export const VocabularyValueSchema = z
  .string()
  .trim()
  .min(1)
  .max(VOCABULARY_VALUE_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > VOCABULARY_VALUE_MAX_LENGTH ||
      utf8ByteLength(value) <= VOCABULARY_VALUE_MAX_UTF8_BYTES,
    'Vocabulary entry is too large when encoded as UTF-8',
  )
  .refine((value) => /[\p{L}\p{N}]/u.test(value), 'Vocabulary must contain a letter or number')
  .refine(
    (value) =>
      !Array.from(value).some((character) => {
        const code = character.codePointAt(0) ?? 0;
        return code < 32 || code === 127;
      }),
    'Control characters are not allowed',
  );
export const VocabularyEntrySchema = z
  .object({
    id: VocabularyIdSchema,
    value: VocabularyValueSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const VocabularyListSchema = z
  .array(VocabularyEntrySchema)
  .max(VOCABULARY_LIMIT)
  .refine((entries) => {
    if (entries.length > VOCABULARY_LIMIT) return true;
    let total = 0;
    for (const entry of entries) {
      if (entry.value.length > VOCABULARY_VALUE_MAX_LENGTH) return true;
      total += utf8ByteLength(entry.value);
      if (total > VOCABULARY_TOTAL_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Custom vocabulary exceeds the total UTF-8 size limit');
export const VocabularyFileResultSchema = z.discriminatedUnion('status', [
  z.object({ status: z.literal('cancelled') }).strict(),
  z.object({ status: z.literal('imported'), count: z.number().int().nonnegative() }).strict(),
  z.object({ status: z.literal('exported'), count: z.number().int().nonnegative() }).strict(),
]);

export type VocabularyEntry = z.infer<typeof VocabularyEntrySchema>;
export type VocabularyFileResult = z.infer<typeof VocabularyFileResultSchema>;
