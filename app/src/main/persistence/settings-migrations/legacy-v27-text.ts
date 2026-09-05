import { z } from 'zod';

// Frozen released contract. Do not import mutable current schemas.
const utf8ByteLength = (value: string): number => new TextEncoder().encode(value).byteLength;
export const noControlCharacters = (value: string): boolean => {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint < 0x20 || codePoint === 0x7f) return false;
  }
  return true;
};
const normalizeCommandText = (value: string): string => {
  const compatibilityLetters: Readonly<Record<string, string>> = {
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
  };
  return Array.from(value)
    .map((character) => compatibilityLetters[character] ?? character)
    .join('')
    .normalize('NFKD')
    .replace(/(?<=\p{Script=Latin})\p{M}+/gu, '')
    .toLocaleLowerCase('en-US')
    .replace(/[\p{Pd}'’ʼ]+/gu, '')
    .replace(/\p{P}+/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim();
};

const VoiceCommandTrigger = z
  .string()
  .trim()
  .min(1)
  .max(200)
  .refine(
    (value) => value.length > 200 || utf8ByteLength(value) <= 400,
    'Trigger is too large when encoded as UTF-8',
  )
  .refine(noControlCharacters, 'Control characters are not allowed')
  .refine(
    (value) => /[\p{L}\p{N}]/u.test(normalizeCommandText(value)),
    'Trigger must contain a letter or number after normalization',
  );
const VoiceCommandSnippet = z
  .string()
  .min(1)
  .max(100_000)
  .refine(
    (value) => value.length > 100_000 || utf8ByteLength(value) <= 200_000,
    'Snippet is too large when encoded as UTF-8',
  )
  .refine((value) => value.trim().length > 0, 'Snippet must contain text')
  .refine(
    (value) =>
      !Array.from(value).some((character) => {
        const code = character.codePointAt(0) ?? 0;
        return code !== 9 && code !== 10 && code !== 13 && (code < 32 || code === 127);
      }),
    'Unsupported control characters are not allowed',
  );
const VoiceCommand = z
  .object({
    id: z.uuid(),
    trigger: VoiceCommandTrigger,
    snippet: VoiceCommandSnippet,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const VoiceCommands = z
  .array(VoiceCommand)
  .max(100)
  .refine((commands) => {
    if (commands.length > 100) return true;
    let total = 0;
    for (const command of commands) {
      if (command.trigger.length > 200 || command.snippet.length > 100_000) return true;
      total += utf8ByteLength(command.trigger) + utf8ByteLength(command.snippet);
      if (total > 512_000) return false;
    }
    return true;
  }, 'Voice commands exceed the total UTF-8 size limit');
const VocabularyValue = z
  .string()
  .trim()
  .min(1)
  .max(200)
  .refine(
    (value) => value.length > 200 || utf8ByteLength(value) <= 400,
    'Vocabulary entry is too large when encoded as UTF-8',
  )
  .refine((value) => /[\p{L}\p{N}]/u.test(value), 'Vocabulary must contain a letter or number')
  .refine(noControlCharacters, 'Control characters are not allowed');
const Vocabulary = z
  .object({
    id: z.uuid(),
    value: VocabularyValue,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const VocabularyList = z
  .array(Vocabulary)
  .max(1_000)
  .refine((entries) => {
    if (entries.length > 1_000) return true;
    let total = 0;
    for (const entry of entries) {
      if (entry.value.length > 200) return true;
      total += utf8ByteLength(entry.value);
      if (total > 256_000) return false;
    }
    return true;
  }, 'Custom vocabulary exceeds the total UTF-8 size limit');
