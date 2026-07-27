import { z } from 'zod';
import {
  LegacyPrivacyV14Schema,
  LegacyRecordingSettingsSchema,
  LegacySmartProcessingSettingsSchema,
  LegacyVocabularyListSchema,
  LegacyVoiceCommandListSchema,
} from './legacy-settings-contracts';
import { LEGACY_WELCOME_V19_V20_FIELDS } from './legacy-settings-v19';

// Frozen snapshot of the released v21 on-disk contract. Keep this file independent from mutable
// current schemas: v21 had two required built-ins, at most ten profiles, free-form nullable
// transcription language, and canonical shortcut chords.
const LegacyProfileIdV21Schema = z.union([z.enum(['general', 'prompt']), z.uuid()]);
const LegacyShortcutKeyV21Schema = z.enum([
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'O',
  'P',
  'Q',
  'R',
  'S',
  'T',
  'U',
  'V',
  'W',
  'X',
  'Y',
  'Z',
]);
const LegacyShortcutV21Schema = z
  .object({
    modifiers: z
      .object({ ctrl: z.boolean(), alt: z.boolean(), shift: z.boolean(), meta: z.boolean() })
      .strict(),
    keys: z
      .array(LegacyShortcutKeyV21Schema)
      .min(1)
      .max(26)
      .superRefine((keys, context) => {
        const seen = new Set<string>();
        for (const [index, key] of keys.entries()) {
          if (seen.has(key)) {
            context.addIssue({
              code: 'custom',
              path: [index],
              message: 'Shortcut keys must be unique',
            });
          }
          seen.add(key);
        }
      }),
  })
  .strict()
  .superRefine((shortcut, context) => {
    const { ctrl, alt, shift, meta } = shortcut.modifiers;
    if (!ctrl && !alt && !shift && !meta) {
      context.addIssue({
        code: 'custom',
        path: ['modifiers'],
        message: 'A shortcut must contain at least one modifier',
      });
    }
  });

const LegacyDictationProfileV21Schema = z
  .object({
    id: LegacyProfileIdV21Schema,
    name: z.string().trim().min(1).max(80),
    shortcut: LegacyShortcutV21Schema,
    processingMode: z.enum(['raw', 'smart']),
    smartPrompt: z.string().trim().max(4_096).nullable(),
  })
  .strict();

export const LEGACY_DEFAULT_GENERAL_PROFILE_V21 = Object.freeze({
  id: 'general',
  name: 'General',
  shortcut: {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys: ['Z'],
  },
  processingMode: 'raw',
  smartPrompt: null,
} as const);
export const LEGACY_DEFAULT_PROMPT_PROFILE_V21 = Object.freeze({
  id: 'prompt',
  name: 'Prompt',
  shortcut: {
    modifiers: { ctrl: false, alt: true, shift: true, meta: false },
    keys: ['Z'],
  },
  processingMode: 'smart',
  smartPrompt:
    'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
} as const);

export const LegacyDictationProfileListV21Schema = z
  .array(LegacyDictationProfileV21Schema)
  .min(2)
  .max(10)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const priorShortcuts: z.infer<typeof LegacyShortcutV21Schema>[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      }
      ids.add(profile.id);
      const reservedOwner =
        profile.shortcut.modifiers.ctrl ||
        !profile.shortcut.modifiers.alt ||
        profile.shortcut.modifiers.meta ||
        profile.shortcut.keys[0] !== 'Z'
          ? null
          : profile.shortcut.modifiers.shift
            ? 'prompt'
            : 'general';
      if (reservedOwner !== null && reservedOwner !== profile.id) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message:
            'The default General and Prompt shortcuts are reserved for their built-in profiles.',
        });
      }
      if (
        priorShortcuts.some((candidate) => legacyShortcutsConflict(candidate, profile.shortcut))
      ) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts with the same modifiers must not prefix one another',
        });
      }
      priorShortcuts.push(profile.shortcut);
    }
    for (const id of ['general', 'prompt']) {
      if (!ids.has(id)) {
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
      }
    }
  });

const LegacyAppSettingsV21Schema = z
  .object({
    enabled: z.boolean(),
    closeToTray: z.boolean(),
    defaultProcessingMode: z.enum(['raw', 'smart']),
    widgetSize: z.enum(['default', 'large', 'huge', 'max']),
    soundsEnabled: z.boolean(),
    launchAtLogin: z.boolean(),
  })
  .strict();
const LegacyTranscriptionSettingsV21Schema = z
  .object({
    modelId: z.enum(['onnx-community/whisper-large-v3-turbo', 'Xenova/whisper-small']),
    language: z.string().trim().min(1).max(80).nullable(),
  })
  .strict();
const LegacyWelcomeSettingsV21Schema = z
  .object({
    ...LEGACY_WELCOME_V19_V20_FIELDS,
    lastStep: z.union([z.literal(1), z.literal(2), z.literal(3), z.literal(4), z.literal(5)]),
  })
  .strict();

export const LegacySettingsV21Schema = z
  .object({
    schemaVersion: z.literal(21),
    app: LegacyAppSettingsV21Schema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsV21Schema,
    dictationProfiles: LegacyDictationProfileListV21Schema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsV21Schema,
  })
  .strict()
  .superRefine((settings, context) => {
    const general = settings.dictationProfiles.find((profile) => profile.id === 'general');
    if (general !== undefined && settings.app.defaultProcessingMode !== general.processingMode) {
      context.addIssue({
        code: 'custom',
        path: ['app', 'defaultProcessingMode'],
        message: 'The processing mode compatibility mirror must match the General profile.',
      });
    }
  });

export type LegacySettingsV21 = z.infer<typeof LegacySettingsV21Schema>;

function legacyShortcutsConflict(
  left: z.infer<typeof LegacyShortcutV21Schema>,
  right: z.infer<typeof LegacyShortcutV21Schema>,
): boolean {
  const leftModifiers = left.modifiers;
  const rightModifiers = right.modifiers;
  if (
    leftModifiers.ctrl !== rightModifiers.ctrl ||
    leftModifiers.alt !== rightModifiers.alt ||
    leftModifiers.shift !== rightModifiers.shift ||
    leftModifiers.meta !== rightModifiers.meta
  ) {
    return false;
  }
  const length = Math.min(left.keys.length, right.keys.length);
  for (let index = 0; index < length; index += 1) {
    if (left.keys[index] !== right.keys[index]) return false;
  }
  return true;
}
