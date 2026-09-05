import { z } from 'zod';
import type { LegacyProviderId, LegacyProviderDraft } from './legacy-provider-contracts';
import type { LegacyVoiceCommand, LegacyVocabularyEntry } from './legacy-text-contracts';
export {
  LegacyProviderIdSchema,
  LegacyProviderIdV14Schema,
  type LegacyProviderId,
  LegacyProviderDraftV2Schema,
  LegacyProviderDraftSchema,
  type LegacyProviderDraft,
  LegacyProviderDraftsV2Schema,
  LegacyProviderDraftsSchema,
  LegacyProviderDraftsV14Schema,
  LegacyCredentialEpochsSchema,
  LegacySmartProcessingPrePiPathSchema,
  LegacySmartProcessingSettingsSchema,
  LegacySmartProcessingV14Schema,
} from './legacy-provider-contracts';
export {
  LegacyLooseVoiceCommandSchema,
  LegacyLooseVoiceCommandListSchema,
  LegacyVoiceCommandSchema,
  LegacyVoiceCommandListSchema,
  type LegacyVoiceCommand,
  LegacyLooseVocabularyEntrySchema,
  LegacyLooseVocabularyListSchema,
  LegacyVocabularyEntrySchema,
  LegacyVocabularyListSchema,
  type LegacyVocabularyEntry,
} from './legacy-text-contracts';

// These schemas are snapshots of released on-disk settings contracts. Do not replace their
// literals, bounds, or refinements with imports from mutable current schemas.
export const LegacyActivationKeySchema = z.enum([
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
export const LegacyProcessingModeSchema = z.enum(['raw', 'smart']);
const LegacyWidgetSizeSchema = z.enum(['default', 'large', 'huge', 'max']);

export const LegacyAppSettingsSchema = z
  .object({ enabled: z.boolean(), closeToTray: z.boolean() })
  .strict();
export const LegacyExtendedAppSettingsSchema = z
  .object({
    enabled: z.boolean(),
    closeToTray: z.boolean(),
    activationKey: LegacyActivationKeySchema,
    defaultProcessingMode: LegacyProcessingModeSchema,
    widgetSize: LegacyWidgetSizeSchema,
    soundsEnabled: z.boolean(),
    launchAtLogin: z.boolean().optional(),
  })
  .strict();
export const LegacyRequiredLaunchAppSettingsSchema = LegacyExtendedAppSettingsSchema.extend({
  launchAtLogin: z.boolean(),
});

const LegacyMicrophoneIdSchema = z.string().min(1).max(1_024);
export const LegacyRecordingSettingsSchema = z
  .object({
    preferredMicrophoneId: LegacyMicrophoneIdSchema.nullable(),
    silencePreset: z.enum(['aggressive', 'average', 'relaxed']),
  })
  .strict();

export const LegacyWhisperModelIdSchema = z.enum([
  'Xenova/whisper-small',
  'Xenova/whisper-large',
  'onnx-community/whisper-large-v3-turbo',
]);
export const LegacyTranscriptionSettingsSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    language: z.string().trim().min(1).max(80).nullable(),
  })
  .strict();

export const LegacyPrivacyV7Schema = z
  .object({
    historyEnabled: z.boolean(),
    historyRetentionDays: z.union([z.literal(7), z.literal(30), z.literal(90)]).nullable(),
  })
  .strict();
export const LegacyPrivacyV10Schema = LegacyPrivacyV7Schema.extend({
  retainSmartScreenshots: z.boolean(),
});
export const LegacyPrivacyV14Schema = LegacyPrivacyV10Schema.extend({
  diagnosticLoggingEnabled: z.boolean(),
});

const LegacyWelcomeStepSchema = z.union([
  z.literal(1),
  z.literal(2),
  z.literal(3),
  z.literal(4),
  z.literal(5),
  z.literal(6),
]);
const LegacyMicrophoneEvidenceSchema = z
  .object({
    boundDeviceId: LegacyMicrophoneIdSchema.nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold, {
    message: 'Microphone evidence must contain a usable signal.',
  });
const LegacyActivationEvidenceSchema = z
  .object({
    activationKey: z.string().length(1),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyModelEvidenceSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyWelcomeSettingsSchema = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: LegacyWelcomeStepSchema,
    microphoneTested: z.boolean(),
    activationTested: z.boolean(),
    microphoneEvidence: LegacyMicrophoneEvidenceSchema.nullable().optional(),
    activationEvidence: LegacyActivationEvidenceSchema.nullable().optional(),
    modelEvidence: LegacyModelEvidenceSchema.nullable().optional(),
    revision: z.number().int().nonnegative().optional(),
  })
  .strict();
// V16 cleared these values before target validation, so its released migration accepted both the
// historical evidence shape and later shapes. Preserve that permissive source behavior.
export const LegacyWelcomeSettingsV16Schema = LegacyWelcomeSettingsSchema.extend({
  activationTested: z.unknown().optional(),
  activationEvidence: z.unknown().nullable().optional(),
  modelEvidence: z.unknown().nullable().optional(),
});

export interface LegacySettingsBase {
  readonly app:
    z.infer<typeof LegacyAppSettingsSchema> | z.infer<typeof LegacyExtendedAppSettingsSchema>;
  readonly recording?: z.infer<typeof LegacyRecordingSettingsSchema>;
  readonly transcription?: z.infer<typeof LegacyTranscriptionSettingsSchema>;
  readonly privacy?: z.infer<typeof LegacyPrivacyV7Schema>;
  readonly voiceCommands?: readonly LegacyVoiceCommand[];
  readonly customVocabulary?: readonly LegacyVocabularyEntry[];
  readonly smartProcessing?: {
    readonly selectedProviderId: LegacyProviderId;
    readonly providers: Partial<Record<LegacyProviderId, LegacyProviderDraft>>;
    readonly credentialEpochs?: Partial<Record<LegacyProviderId, number>>;
    readonly onScreenAwarenessEnabled?: boolean;
    readonly visionOverrides?: readonly {
      readonly providerId: 'generic-openai' | 'litellm';
      readonly binding: string;
      readonly modelId: string;
      readonly verifiedAt: number;
    }[];
  };
}

export interface LegacySettingsWithWelcomeProgress extends LegacySettingsBase {
  readonly welcome: {
    readonly completedAt: number | null;
    readonly lastStep: 1 | 2 | 3 | 4 | 5 | 6;
  };
}
