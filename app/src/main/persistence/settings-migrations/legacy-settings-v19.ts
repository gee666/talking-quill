import { z } from 'zod';
import {
  LegacyActivationKeySchema,
  LegacyPrivacyV14Schema,
  LegacyRecordingSettingsSchema,
  LegacyRequiredLaunchAppSettingsSchema,
  LegacySmartProcessingSettingsSchema,
  LegacyVocabularyListSchema,
  LegacyVoiceCommandListSchema,
} from './legacy-settings-contracts';

// These are frozen snapshots of the released v19 settings contract. Keep them independent from
// mutable current schemas so later shortcut changes cannot alter historical parsing.
const LegacyProfileIdSchema = z.union([z.enum(['general', 'prompt']), z.uuid()]);
const LegacyDictationProfileSchema = z
  .object({
    id: LegacyProfileIdSchema,
    name: z.string().trim().min(1).max(80),
    activationKey: LegacyActivationKeySchema,
    shift: z.boolean(),
    processingMode: z.enum(['raw', 'smart']),
    smartPrompt: z.string().trim().max(4_096).nullable(),
  })
  .strict();

export const LegacyDictationProfileListSchema = z
  .array(LegacyDictationProfileSchema)
  .min(2)
  .max(10)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const bindings = new Set<string>();
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
        profile.activationKey !== 'Z' ? null : profile.shift ? 'prompt' : 'general';
      if (reservedOwner !== null && reservedOwner !== profile.id) {
        context.addIssue({
          code: 'custom',
          path: [index, 'activationKey'],
          message:
            'The default General and Prompt shortcuts are reserved for their built-in profiles.',
        });
      }
      const binding = `${profile.shift ? 'shift+' : ''}${profile.activationKey}`;
      if (bindings.has(binding)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'activationKey'],
          message: 'Profile shortcuts must be distinct',
        });
      }
      bindings.add(binding);
    }
    for (const id of ['general', 'prompt']) {
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
    }
  });

const LegacyWhisperModelIdV19Schema = z.enum([
  'onnx-community/whisper-large-v3-turbo',
  'Xenova/whisper-small',
]);
const LegacyTranscriptionSettingsV19Schema = z
  .object({
    modelId: LegacyWhisperModelIdV19Schema,
    language: z.string().trim().min(1).max(80).nullable(),
  })
  .strict();
const LegacyMicrophoneEvidenceV19Schema = z
  .object({
    boundDeviceId: z.string().min(1).max(1_024).nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold, {
    message: 'Microphone evidence must contain a usable signal.',
  });
const LegacyActivationEvidenceV19Schema = z
  .object({
    profileId: LegacyProfileIdSchema,
    activationKey: LegacyActivationKeySchema,
    shift: z.boolean(),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyModelEvidenceV19Schema = z
  .object({
    modelId: LegacyWhisperModelIdV19Schema,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();

export const LEGACY_WELCOME_V19_V20_FIELDS = Object.freeze({
  completedAt: z.number().int().nonnegative().nullable(),
  microphoneTested: z.boolean(),
  activationTested: z.boolean(),
  microphoneEvidence: LegacyMicrophoneEvidenceV19Schema.nullable().optional(),
  activationEvidence: LegacyActivationEvidenceV19Schema.nullable().optional(),
  modelEvidence: LegacyModelEvidenceV19Schema.nullable().optional(),
  revision: z.number().int().nonnegative().optional(),
});

const LegacyWelcomeSettingsV19Schema = z
  .object({
    ...LEGACY_WELCOME_V19_V20_FIELDS,
    lastStep: z.union([
      z.literal(1),
      z.literal(2),
      z.literal(3),
      z.literal(4),
      z.literal(5),
      z.literal(6),
    ]),
  })
  .strict();

export const LEGACY_SETTINGS_V19_V20_FIELDS = Object.freeze({
  app: LegacyRequiredLaunchAppSettingsSchema,
  recording: LegacyRecordingSettingsSchema,
  transcription: LegacyTranscriptionSettingsV19Schema,
  dictationProfiles: LegacyDictationProfileListSchema,
  privacy: LegacyPrivacyV14Schema,
  smartProcessing: LegacySmartProcessingSettingsSchema,
  voiceCommands: LegacyVoiceCommandListSchema,
  customVocabulary: LegacyVocabularyListSchema,
});

export const LegacySettingsV19Schema = z
  .object({
    schemaVersion: z.literal(19),
    ...LEGACY_SETTINGS_V19_V20_FIELDS,
    welcome: LegacyWelcomeSettingsV19Schema,
  })
  .strict()
  .superRefine(validateLegacySettingsMirrors);

export type LegacySettingsV19 = z.infer<typeof LegacySettingsV19Schema>;

export function validateLegacySettingsMirrors(
  settings: {
    readonly app: { readonly activationKey: string; readonly defaultProcessingMode: string };
    readonly dictationProfiles: readonly {
      readonly id: string;
      readonly activationKey: string;
      readonly processingMode: string;
    }[];
  },
  context: z.core.$RefinementCtx,
): void {
  const general = settings.dictationProfiles.find((profile) => profile.id === 'general');
  if (general === undefined) return;
  if (settings.app.activationKey !== general.activationKey) {
    context.addIssue({
      code: 'custom',
      path: ['app', 'activationKey'],
      message: 'The activation key compatibility mirror must match the General profile.',
    });
  }
  if (settings.app.defaultProcessingMode !== general.processingMode) {
    context.addIssue({
      code: 'custom',
      path: ['app', 'defaultProcessingMode'],
      message: 'The processing mode compatibility mirror must match the General profile.',
    });
  }
}
