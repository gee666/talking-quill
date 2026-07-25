import { z } from 'zod';
import { ActivationKeySchema, type ActivationKey } from '../helper/protocol';
import { ProcessingModeSchema } from './history';

export const MAX_DICTATION_PROFILES = 10;
export const GENERAL_PROFILE_ID = 'general' as const;
export const PROMPT_PROFILE_ID = 'prompt' as const;
export const BuiltInDictationProfileIdSchema = z.enum([GENERAL_PROFILE_ID, PROMPT_PROFILE_ID]);
export const CustomDictationProfileIdSchema = z.uuid();
export const DictationProfileIdSchema = z.union([
  BuiltInDictationProfileIdSchema,
  CustomDictationProfileIdSchema,
]);
export const DictationProfileNameSchema = z.string().trim().min(1).max(80);
export const DictationProfileSmartPromptSchema = z.string().trim().max(4_096).nullable();

export const RESERVED_DICTATION_BINDINGS = Object.freeze([
  { ownerId: GENERAL_PROFILE_ID, activationKey: 'Z' as const, shift: false },
  { ownerId: PROMPT_PROFILE_ID, activationKey: 'Z' as const, shift: true },
]);
export const RESERVED_DICTATION_BINDING_ERROR =
  'The default General and Prompt shortcuts are reserved for their built-in profiles.';

export function reservedBindingOwner(
  activationKey: ActivationKey,
  shift: boolean,
): BuiltInDictationProfileId | null {
  return (
    RESERVED_DICTATION_BINDINGS.find(
      (binding) => binding.activationKey === activationKey && binding.shift === shift,
    )?.ownerId ?? null
  );
}

export function isReservedBindingForAnotherProfile(
  id: string,
  activationKey: ActivationKey,
  shift: boolean,
): boolean {
  const owner = reservedBindingOwner(activationKey, shift);
  return owner !== null && owner !== id;
}

export const DictationProfileSchema = z
  .object({
    id: DictationProfileIdSchema,
    name: DictationProfileNameSchema,
    activationKey: ActivationKeySchema,
    shift: z.boolean(),
    processingMode: ProcessingModeSchema,
    smartPrompt: DictationProfileSmartPromptSchema,
  })
  .strict();

export const DictationProfileListSchema = z
  .array(DictationProfileSchema)
  .min(2)
  .max(MAX_DICTATION_PROFILES)
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
      if (isReservedBindingForAnotherProfile(profile.id, profile.activationKey, profile.shift)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'activationKey'],
          message: RESERVED_DICTATION_BINDING_ERROR,
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
    for (const id of [GENERAL_PROFILE_ID, PROMPT_PROFILE_ID]) {
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
    }
  });

const DictationProfileInputSchema = DictationProfileSchema.omit({ id: true });
export const DictationProfileCreateSchema = DictationProfileInputSchema.refine(
  (profile) => reservedBindingOwner(profile.activationKey, profile.shift) === null,
  { path: ['activationKey'], message: RESERVED_DICTATION_BINDING_ERROR },
);
const DictationProfileNonBindingPatchSchema = DictationProfileInputSchema.pick({
  name: true,
  processingMode: true,
  smartPrompt: true,
})
  .partial()
  .strict()
  .refine((patch) => Object.keys(patch).length > 0, {
    message: 'A profile patch must not be empty',
  });
const DictationProfileBindingPatchSchema = z
  .object({
    activationKey: ActivationKeySchema,
    shift: z.boolean(),
    name: DictationProfileNameSchema.optional(),
    processingMode: ProcessingModeSchema.optional(),
    smartPrompt: DictationProfileSmartPromptSchema.optional(),
  })
  .strict();
export const DictationProfilePatchSchema = z.union([
  DictationProfileBindingPatchSchema,
  DictationProfileNonBindingPatchSchema,
]);

export type BuiltInDictationProfileId = z.infer<typeof BuiltInDictationProfileIdSchema>;
export type CustomDictationProfileId = z.infer<typeof CustomDictationProfileIdSchema>;
export type DictationProfileId = z.infer<typeof DictationProfileIdSchema>;
export type DictationProfile = z.infer<typeof DictationProfileSchema>;
export type DictationProfileCreate = z.infer<typeof DictationProfileCreateSchema>;
export type DictationProfilePatch = z.infer<typeof DictationProfilePatchSchema>;

export const DEFAULT_GENERAL_PROFILE: DictationProfile = Object.freeze({
  id: GENERAL_PROFILE_ID,
  name: 'General',
  activationKey: 'Z',
  shift: false,
  processingMode: 'raw',
  smartPrompt: null,
});
export const DEFAULT_PROMPT_PROFILE: DictationProfile = Object.freeze({
  id: PROMPT_PROFILE_ID,
  name: 'Prompt',
  activationKey: 'Z',
  shift: true,
  processingMode: 'smart',
  smartPrompt:
    'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
});

export function defaultDictationProfiles(): DictationProfile[] {
  return [structuredClone(DEFAULT_GENERAL_PROFILE), structuredClone(DEFAULT_PROMPT_PROFILE)];
}
