import { z } from 'zod';
import { ShortcutKeySchema } from './shortcut';
import { MicrophoneIdSchema } from './audio';
import { DictationProfileIdSchema } from './dictation-profiles';
import { WhisperModelIdSchema } from './model-manifest';

export const WelcomeStepSchema = z.union([
  z.literal(1),
  z.literal(2),
  z.literal(3),
  z.literal(4),
  z.literal(5),
]);

export const MicrophoneEvidenceSchema = z
  .object({
    boundDeviceId: MicrophoneIdSchema.nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold, {
    message: 'Microphone evidence must contain a usable signal.',
  });

export const ActivationEvidenceSchema = z
  .object({
    profileId: DictationProfileIdSchema,
    activationKey: ShortcutKeySchema,
    shift: z.boolean(),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();

export const ModelEvidenceSchema = z
  .object({
    modelId: WhisperModelIdSchema,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();

export const WelcomeSettingsSchema = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: WelcomeStepSchema,
    microphoneTested: z.boolean(),
    // Retained only for settings compatibility. Activation testing is not an onboarding step.
    activationTested: z.boolean(),
    microphoneEvidence: MicrophoneEvidenceSchema.nullable().optional(),
    activationEvidence: ActivationEvidenceSchema.nullable().optional(),
    modelEvidence: ModelEvidenceSchema.nullable().optional(),
    revision: z.number().int().nonnegative().optional(),
  })
  .strict();

export const WelcomeStateSchema = WelcomeSettingsSchema.extend({
  reopened: z.boolean(),
}).strict();

export type WelcomeStep = z.infer<typeof WelcomeStepSchema>;
export type WelcomeSettings = z.infer<typeof WelcomeSettingsSchema>;
export type WelcomeState = z.infer<typeof WelcomeStateSchema>;
