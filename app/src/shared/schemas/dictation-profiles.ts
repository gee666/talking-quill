import { z } from 'zod';
import { ProcessingModeSchema } from './history';
import {
  ShortcutSchema,
  shortcutFromLegacyActivation,
  shortcutIdentity,
  shortcutsConflict,
  type Shortcut,
} from './shortcut';

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

export const DEFAULT_GENERAL_SHORTCUT = deepFreeze(shortcutFromLegacyActivation('Z', false));
export const DEFAULT_PROMPT_SHORTCUT = deepFreeze(shortcutFromLegacyActivation('Z', true));

export const RESERVED_DICTATION_BINDINGS = deepFreeze([
  { ownerId: GENERAL_PROFILE_ID, shortcut: DEFAULT_GENERAL_SHORTCUT },
  { ownerId: PROMPT_PROFILE_ID, shortcut: DEFAULT_PROMPT_SHORTCUT },
]);
export const RESERVED_DICTATION_BINDING_ERROR =
  'The default General and Prompt shortcuts are reserved for their built-in profiles.';

export function reservedBindingOwner(shortcut: Shortcut): BuiltInDictationProfileId | null {
  return (
    RESERVED_DICTATION_BINDINGS.find((binding) => shortcutsConflict(binding.shortcut, shortcut))
      ?.ownerId ?? null
  );
}

export function isReservedBindingForAnotherProfile(id: string, shortcut: Shortcut): boolean {
  const owner = reservedBindingOwner(shortcut);
  return owner !== null && owner !== id;
}

export const DictationProfileSchema = z
  .object({
    id: DictationProfileIdSchema,
    name: DictationProfileNameSchema,
    shortcut: ShortcutSchema,
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
    const shortcutIdentities = new Set<string>();
    const priorShortcuts: Shortcut[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      }
      ids.add(profile.id);
      if (isReservedBindingForAnotherProfile(profile.id, profile.shortcut)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: RESERVED_DICTATION_BINDING_ERROR,
        });
      }
      const identity = shortcutIdentity(profile.shortcut);
      if (shortcutIdentities.has(identity)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts must be distinct',
        });
      } else if (priorShortcuts.some((shortcut) => shortcutsConflict(shortcut, profile.shortcut))) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts with the same modifiers must not prefix one another',
        });
      }
      shortcutIdentities.add(identity);
      priorShortcuts.push(profile.shortcut);
    }
    for (const id of [GENERAL_PROFILE_ID, PROMPT_PROFILE_ID]) {
      if (!ids.has(id)) {
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
      }
    }
  });

const DictationProfileInputSchema = DictationProfileSchema.omit({ id: true });
export const DictationProfileCreateSchema = DictationProfileInputSchema.refine(
  (profile) => reservedBindingOwner(profile.shortcut) === null,
  { path: ['shortcut'], message: RESERVED_DICTATION_BINDING_ERROR },
);
export const DictationProfilePatchSchema = DictationProfileInputSchema.partial()
  .strict()
  .refine((patch) => Object.keys(patch).length > 0, {
    message: 'A profile patch must not be empty',
  });

export type BuiltInDictationProfileId = z.infer<typeof BuiltInDictationProfileIdSchema>;
export type CustomDictationProfileId = z.infer<typeof CustomDictationProfileIdSchema>;
export type DictationProfileId = z.infer<typeof DictationProfileIdSchema>;
export type DictationProfile = z.infer<typeof DictationProfileSchema>;
export type DictationProfileCreate = z.infer<typeof DictationProfileCreateSchema>;
export type DictationProfilePatch = z.infer<typeof DictationProfilePatchSchema>;

export const DEFAULT_GENERAL_PROFILE: DictationProfile = deepFreeze({
  id: GENERAL_PROFILE_ID,
  name: 'General',
  shortcut: DEFAULT_GENERAL_SHORTCUT,
  processingMode: 'raw',
  smartPrompt: null,
});
export const DEFAULT_PROMPT_PROFILE: DictationProfile = deepFreeze({
  id: PROMPT_PROFILE_ID,
  name: 'Prompt',
  shortcut: DEFAULT_PROMPT_SHORTCUT,
  processingMode: 'smart',
  smartPrompt:
    'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
});

export function defaultDictationProfiles(): DictationProfile[] {
  return [structuredClone(DEFAULT_GENERAL_PROFILE), structuredClone(DEFAULT_PROMPT_PROFILE)];
}

function deepFreeze<Value>(value: Value): Value {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const nested of Object.values(value)) deepFreeze(nested);
  }
  return value;
}
