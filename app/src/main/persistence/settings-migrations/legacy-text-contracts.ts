import { z } from 'zod';

// Frozen released contract. Do not import mutable current schemas.
const LEGACY_VOICE_COMMAND_LIMIT = 100;
const LEGACY_VOICE_TRIGGER_MAX_LENGTH = 200;
const LEGACY_VOICE_TRIGGER_MAX_UTF8_BYTES = 400;
const LEGACY_VOICE_SNIPPET_MAX_LENGTH = 100_000;
const LEGACY_VOICE_SNIPPET_MAX_UTF8_BYTES = 200_000;
const LEGACY_VOICE_COMMANDS_MAX_UTF8_BYTES = 512_000;

export const LegacyLooseVoiceCommandSchema = z
  .object({
    id: z.uuid(),
    trigger: z.string().trim().min(1).max(LEGACY_VOICE_TRIGGER_MAX_LENGTH),
    snippet: z.string().min(1).max(LEGACY_VOICE_SNIPPET_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyLooseVoiceCommandListSchema = z
  .array(LegacyLooseVoiceCommandSchema)
  .max(LEGACY_VOICE_COMMAND_LIMIT);

const LegacyVoiceCommandTriggerSchema = z
  .string()
  .trim()
  .min(1)
  .max(LEGACY_VOICE_TRIGGER_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOICE_TRIGGER_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOICE_TRIGGER_MAX_UTF8_BYTES,
    'Trigger is too large when encoded as UTF-8',
  )
  .refine(
    (value) => !hasLegacyControlCharacters(value, false),
    'Control characters are not allowed',
  )
  .refine(
    (value) => /[\p{L}\p{N}]/u.test(normalizeLegacyCommandText(value)),
    'Trigger must contain a letter or number after normalization',
  );
const LegacyVoiceCommandSnippetSchema = z
  .string()
  .min(1)
  .max(LEGACY_VOICE_SNIPPET_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOICE_SNIPPET_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOICE_SNIPPET_MAX_UTF8_BYTES,
    'Snippet is too large when encoded as UTF-8',
  )
  .refine((value) => value.trim().length > 0, 'Snippet must contain text')
  .refine(
    (value) => !hasLegacyControlCharacters(value, true),
    'Unsupported control characters are not allowed',
  );
export const LegacyVoiceCommandSchema = z
  .object({
    id: z.uuid(),
    trigger: LegacyVoiceCommandTriggerSchema,
    snippet: LegacyVoiceCommandSnippetSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyVoiceCommandListSchema = z
  .array(LegacyVoiceCommandSchema)
  .max(LEGACY_VOICE_COMMAND_LIMIT)
  .refine((commands) => {
    if (commands.length > LEGACY_VOICE_COMMAND_LIMIT) return true;
    let total = 0;
    for (const command of commands) {
      if (
        command.trigger.length > LEGACY_VOICE_TRIGGER_MAX_LENGTH ||
        command.snippet.length > LEGACY_VOICE_SNIPPET_MAX_LENGTH
      ) {
        return true;
      }
      total += legacyUtf8ByteLength(command.trigger) + legacyUtf8ByteLength(command.snippet);
      if (total > LEGACY_VOICE_COMMANDS_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Voice commands exceed the total UTF-8 size limit');
export type LegacyVoiceCommand = z.infer<typeof LegacyLooseVoiceCommandSchema>;

const LEGACY_VOCABULARY_LIMIT = 1_000;
const LEGACY_VOCABULARY_VALUE_MAX_LENGTH = 200;
const LEGACY_VOCABULARY_VALUE_MAX_UTF8_BYTES = 400;
const LEGACY_VOCABULARY_TOTAL_MAX_UTF8_BYTES = 256_000;

export const LegacyLooseVocabularyEntrySchema = z
  .object({
    id: z.uuid(),
    value: z.string().trim().min(1).max(LEGACY_VOCABULARY_VALUE_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyLooseVocabularyListSchema = z
  .array(LegacyLooseVocabularyEntrySchema)
  .max(LEGACY_VOCABULARY_LIMIT);
const LegacyVocabularyValueSchema = z
  .string()
  .trim()
  .min(1)
  .max(LEGACY_VOCABULARY_VALUE_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOCABULARY_VALUE_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOCABULARY_VALUE_MAX_UTF8_BYTES,
    'Vocabulary entry is too large when encoded as UTF-8',
  )
  .refine((value) => /[\p{L}\p{N}]/u.test(value), 'Vocabulary must contain a letter or number')
  .refine(
    (value) => !hasLegacyControlCharacters(value, false),
    'Control characters are not allowed',
  );
export const LegacyVocabularyEntrySchema = z
  .object({
    id: z.uuid(),
    value: LegacyVocabularyValueSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyVocabularyListSchema = z
  .array(LegacyVocabularyEntrySchema)
  .max(LEGACY_VOCABULARY_LIMIT)
  .refine((entries) => {
    if (entries.length > LEGACY_VOCABULARY_LIMIT) return true;
    let total = 0;
    for (const entry of entries) {
      if (entry.value.length > LEGACY_VOCABULARY_VALUE_MAX_LENGTH) return true;
      total += legacyUtf8ByteLength(entry.value);
      if (total > LEGACY_VOCABULARY_TOTAL_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Custom vocabulary exceeds the total UTF-8 size limit');
export type LegacyVocabularyEntry = z.infer<typeof LegacyLooseVocabularyEntrySchema>;

const LEGACY_COMPATIBILITY_LETTERS: Readonly<Record<string, string>> = Object.freeze({
  Ł: 'L',
  ł: 'l',
  Đ: 'D',
  đ: 'd',
  Ø: 'O',
  ø: 'o',
  Æ: 'AE',
  æ: 'ae',
  Œ: 'OE',
  œ: 'oe',
  Ð: 'D',
  ð: 'd',
  Þ: 'TH',
  þ: 'th',
});

function normalizeLegacyCommandText(value: string): string {
  return Array.from(value)
    .map((character) => LEGACY_COMPATIBILITY_LETTERS[character] ?? character)
    .join('')
    .normalize('NFKD')
    .replace(/(?<=\p{Script=Latin})\p{M}+/gu, '')
    .toLocaleLowerCase('en-US')
    .replace(/[\p{Pd}'’ʼ]+/gu, '')
    .replace(/\p{P}+/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim();
}

function hasLegacyControlCharacters(value: string, allowWhitespace: boolean): boolean {
  return Array.from(value).some((character) => {
    const code = character.codePointAt(0) ?? 0;
    if (allowWhitespace && (code === 9 || code === 10 || code === 13)) return false;
    return code < 32 || code === 127;
  });
}

function legacyUtf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}
