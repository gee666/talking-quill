import { z } from 'zod';
import { isMeaningfulCommandText } from '../text/command-normalization';
import { utf8ByteLength } from './text-bounds';

export const VOICE_COMMAND_LIMIT = 100;
export const VOICE_TRIGGER_MAX_LENGTH = 200;
export const VOICE_TRIGGER_MAX_UTF8_BYTES = 400;
export const VOICE_SNIPPET_MAX_LENGTH = 100_000;
export const VOICE_SNIPPET_MAX_UTF8_BYTES = 200_000;
export const VOICE_COMMANDS_MAX_UTF8_BYTES = 512_000;

const singleLineText = z
  .string()
  .trim()
  .min(1)
  .max(VOICE_TRIGGER_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > VOICE_TRIGGER_MAX_LENGTH ||
      utf8ByteLength(value) <= VOICE_TRIGGER_MAX_UTF8_BYTES,
    'Trigger is too large when encoded as UTF-8',
  )
  .refine((value) => !hasControlCharacters(value, false), 'Control characters are not allowed')
  .refine(isMeaningfulCommandText, 'Trigger must contain a letter or number after normalization');

export const VoiceCommandIdSchema = z.uuid();
export const VoiceCommandTriggerSchema = singleLineText;
export const VoiceCommandSnippetSchema = z
  .string()
  .min(1)
  .max(VOICE_SNIPPET_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > VOICE_SNIPPET_MAX_LENGTH ||
      utf8ByteLength(value) <= VOICE_SNIPPET_MAX_UTF8_BYTES,
    'Snippet is too large when encoded as UTF-8',
  )
  .refine((value) => value.trim().length > 0, 'Snippet must contain text')
  .refine(
    (value) => !hasControlCharacters(value, true),
    'Unsupported control characters are not allowed',
  );

export const VoiceCommandSchema = z
  .object({
    id: VoiceCommandIdSchema,
    trigger: VoiceCommandTriggerSchema,
    snippet: VoiceCommandSnippetSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();

export const VoiceCommandInputSchema = z
  .object({ trigger: VoiceCommandTriggerSchema, snippet: VoiceCommandSnippetSchema })
  .strict();
export const VoiceCommandUpdateSchema = VoiceCommandInputSchema.partial().refine(
  (value) => Object.keys(value).length > 0,
  'At least one command field must be updated',
);
export const VoiceCommandListSchema = z
  .array(VoiceCommandSchema)
  .max(VOICE_COMMAND_LIMIT)
  .refine((commands) => {
    if (commands.length > VOICE_COMMAND_LIMIT) return true;
    let total = 0;
    for (const command of commands) {
      if (
        command.trigger.length > VOICE_TRIGGER_MAX_LENGTH ||
        command.snippet.length > VOICE_SNIPPET_MAX_LENGTH
      ) {
        return true;
      }
      total += utf8ByteLength(command.trigger) + utf8ByteLength(command.snippet);
      if (total > VOICE_COMMANDS_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Voice commands exceed the total UTF-8 size limit');
export const VoiceCommandMatchSchema = z
  .object({
    command: VoiceCommandSchema,
    kind: z.enum(['exact', 'fuzzy']),
    score: z.number().min(0).max(1),
  })
  .strict();

export type VoiceCommand = z.infer<typeof VoiceCommandSchema>;
export type VoiceCommandInput = z.infer<typeof VoiceCommandInputSchema>;
export type VoiceCommandUpdate = z.infer<typeof VoiceCommandUpdateSchema>;
export type VoiceCommandMatch = z.infer<typeof VoiceCommandMatchSchema>;

function hasControlCharacters(value: string, allowWhitespace: boolean): boolean {
  return Array.from(value).some((character) => {
    const code = character.codePointAt(0) ?? 0;
    if (allowWhitespace && (code === 9 || code === 10 || code === 13)) return false;
    return code < 32 || code === 127;
  });
}
