import { z } from 'zod';
import { ProcessingModeSchema } from './history';
import {
  ShortcutSchema,
  shortcutIdentity,
  shortcutsConflict,
  shortcutsEqual,
  type Shortcut,
  type ShortcutKey,
} from './shortcut';

export const GENERAL_PROFILE_ID = 'general' as const;
export const PROMPT_PROFILE_ID = 'prompt' as const;
export const MARKDOWN_PROFILE_ID = 'markdown' as const;
export const PROMPT_TO_ENGLISH_PROFILE_ID = 'prompt-to-english' as const;
export const TRANSLATE_TO_ENGLISH_PROFILE_ID = 'translate-to-english' as const;
export const BUILT_IN_DICTATION_PROFILE_IDS = [
  GENERAL_PROFILE_ID,
  PROMPT_PROFILE_ID,
  PROMPT_TO_ENGLISH_PROFILE_ID,
  MARKDOWN_PROFILE_ID,
  TRANSLATE_TO_ENGLISH_PROFILE_ID,
] as const;
export const MAX_CUSTOM_DICTATION_PROFILES = 8;
export const MAX_DICTATION_PROFILES =
  BUILT_IN_DICTATION_PROFILE_IDS.length + MAX_CUSTOM_DICTATION_PROFILES;

export const BuiltInDictationProfileIdSchema = z.enum(BUILT_IN_DICTATION_PROFILE_IDS);
export const CustomDictationProfileIdSchema = z.uuid();
export const DictationProfileIdSchema = z.union([
  BuiltInDictationProfileIdSchema,
  CustomDictationProfileIdSchema,
]);
export const DictationProfileNameSchema = z.string().trim().min(1).max(80);
export const DictationProfileSmartPromptSchema = z.string().trim().max(4_096).nullable();

export const DEFAULT_GENERAL_SHORTCUT = builtInAltShortcut(['X']);
export const DEFAULT_PROMPT_SHORTCUT = builtInAltShortcut(['X', 'P']);
export const DEFAULT_PROMPT_TO_ENGLISH_SHORTCUT = builtInAltShortcut(['X', 'Q']);
export const DEFAULT_MARKDOWN_SHORTCUT = builtInAltShortcut(['X', 'M']);
export const DEFAULT_TRANSLATE_TO_ENGLISH_SHORTCUT = builtInAltShortcut(['X', 'T']);

export const BUILT_IN_DICTATION_PROFILE_METADATA = deepFreeze([
  {
    id: GENERAL_PROFILE_ID,
    description: 'Default: cleans up and formats the transcript in its source language.',
    defaultProfile: {
      id: GENERAL_PROFILE_ID,
      name: 'General',
      shortcut: DEFAULT_GENERAL_SHORTCUT,
      processingMode: 'smart',
      smartPrompt: 'Clean up and format the transcript while preserving its source language.',
    },
  },
  {
    id: PROMPT_PROFILE_ID,
    description:
      'Default: turns dictated ideas into a concise, structured prompt in the source language.',
    defaultProfile: {
      id: PROMPT_PROFILE_ID,
      name: 'Prompt',
      shortcut: DEFAULT_PROMPT_SHORTCUT,
      processingMode: 'smart',
      smartPrompt:
        'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Preserve the source language. Organize the result into clear paragraphs and lists when useful, and use tables or other formatting when helpful.',
    },
  },
  {
    id: PROMPT_TO_ENGLISH_PROFILE_ID,
    description: 'Default: turns dictated ideas into a concise, structured English prompt.',
    defaultProfile: {
      id: PROMPT_TO_ENGLISH_PROFILE_ID,
      name: 'Prompt to English',
      shortcut: DEFAULT_PROMPT_TO_ENGLISH_SHORTCUT,
      processingMode: 'smart',
      smartPrompt:
        'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Translate the result to natural English while preserving its meaning, tone, facts, names, numbers, and level of detail. Organize the result into clear paragraphs and lists when useful, and use tables or other formatting when helpful.',
    },
  },
  {
    id: MARKDOWN_PROFILE_ID,
    description: 'Default: formats the transcript as Markdown in its source language.',
    defaultProfile: {
      id: MARKDOWN_PROFILE_ID,
      name: 'Markdown',
      shortcut: DEFAULT_MARKDOWN_SHORTCUT,
      processingMode: 'smart',
      smartPrompt:
        'Format the transcript as clear Markdown using headings, paragraphs, and lists where useful. Preserve its source language.',
    },
  },
  {
    id: TRANSLATE_TO_ENGLISH_PROFILE_ID,
    description: 'Default: cleans up the transcript and translates it to English.',
    defaultProfile: {
      id: TRANSLATE_TO_ENGLISH_PROFILE_ID,
      name: 'Translate to English',
      shortcut: DEFAULT_TRANSLATE_TO_ENGLISH_SHORTCUT,
      processingMode: 'smart',
      smartPrompt:
        'Translate the transcript to natural English while preserving its meaning, tone, facts, names, numbers, and level of detail.',
    },
  },
] as const);

export const RESERVED_DICTATION_BINDINGS = deepFreeze(
  BUILT_IN_DICTATION_PROFILE_METADATA.map(({ id, defaultProfile }) => ({
    ownerId: id,
    shortcut: defaultProfile.shortcut,
  })),
);
export const RESERVED_DICTATION_BINDING_ERROR =
  'The default built-in profile shortcuts are reserved for their owners.';

export function isBuiltInDefaultBinding(id: string, shortcut: Shortcut): boolean {
  const metadata = builtInDictationProfileMetadata(id);
  return metadata !== null && shortcutsEqual(metadata.defaultProfile.shortcut, shortcut);
}

export function builtInDefaultPrefixConflictAllowed(
  leftId: string,
  leftShortcut: Shortcut,
  rightId: string,
  rightShortcut: Shortcut,
): boolean {
  return (
    leftId !== rightId &&
    isBuiltInDefaultBinding(leftId, leftShortcut) &&
    isBuiltInDefaultBinding(rightId, rightShortcut)
  );
}

export function dictationProfileBindingsConflict(
  leftId: string,
  leftShortcut: Shortcut,
  rightId: string,
  rightShortcut: Shortcut,
): boolean {
  return (
    shortcutsConflict(leftShortcut, rightShortcut) &&
    !builtInDefaultPrefixConflictAllowed(leftId, leftShortcut, rightId, rightShortcut)
  );
}

export function reservedBindingOwner(shortcut: Shortcut): BuiltInDictationProfileId | null {
  const exact = RESERVED_DICTATION_BINDINGS.find((binding) =>
    shortcutsEqual(binding.shortcut, shortcut),
  );
  return (
    exact?.ownerId ??
    RESERVED_DICTATION_BINDINGS.find((binding) => shortcutsConflict(binding.shortcut, shortcut))
      ?.ownerId ??
    null
  );
}

export function isReservedBindingForProfile(id: string, shortcut: Shortcut): boolean {
  if (isBuiltInDefaultBinding(id, shortcut)) return false;
  return RESERVED_DICTATION_BINDINGS.some((binding) =>
    shortcutsConflict(binding.shortcut, shortcut),
  );
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
  .min(BUILT_IN_DICTATION_PROFILE_IDS.length)
  .max(MAX_DICTATION_PROFILES)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const shortcutIdentities = new Set<string>();
    const priorProfiles: { readonly id: string; readonly shortcut: Shortcut }[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      }
      ids.add(profile.id);
      if (isReservedBindingForProfile(profile.id, profile.shortcut)) {
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
      } else if (
        priorProfiles.some((candidate) =>
          dictationProfileBindingsConflict(
            candidate.id,
            candidate.shortcut,
            profile.id,
            profile.shortcut,
          ),
        )
      ) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message:
            'Profile shortcuts with the same modifiers must not prefix one another outside the built-in default family',
        });
      }
      shortcutIdentities.add(identity);
      priorProfiles.push(profile);
    }
    for (const id of BUILT_IN_DICTATION_PROFILE_IDS) {
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
export type BuiltInDictationProfileMetadata = (typeof BUILT_IN_DICTATION_PROFILE_METADATA)[number];
export type CustomDictationProfileId = z.infer<typeof CustomDictationProfileIdSchema>;
export type DictationProfileId = z.infer<typeof DictationProfileIdSchema>;
export type DictationProfile = z.infer<typeof DictationProfileSchema>;
export type DictationProfileCreate = z.infer<typeof DictationProfileCreateSchema>;
export type DictationProfilePatch = z.infer<typeof DictationProfilePatchSchema>;

export const DEFAULT_GENERAL_PROFILE = BUILT_IN_DICTATION_PROFILE_METADATA[0]
  .defaultProfile as DictationProfile;
export const DEFAULT_PROMPT_PROFILE = BUILT_IN_DICTATION_PROFILE_METADATA[1]
  .defaultProfile as DictationProfile;
export const DEFAULT_PROMPT_TO_ENGLISH_PROFILE = BUILT_IN_DICTATION_PROFILE_METADATA[2]
  .defaultProfile as DictationProfile;
export const DEFAULT_MARKDOWN_PROFILE = BUILT_IN_DICTATION_PROFILE_METADATA[3]
  .defaultProfile as DictationProfile;
export const DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE = BUILT_IN_DICTATION_PROFILE_METADATA[4]
  .defaultProfile as DictationProfile;

export function builtInDictationProfileMetadata(
  id: string,
): BuiltInDictationProfileMetadata | null {
  return BUILT_IN_DICTATION_PROFILE_METADATA.find((metadata) => metadata.id === id) ?? null;
}

export function builtInDictationProfile(id: string): DictationProfile | null {
  return builtInDictationProfileMetadata(id)?.defaultProfile ?? null;
}

export function builtInDictationProfileName(id: BuiltInDictationProfileId): string {
  const metadata = builtInDictationProfileMetadata(id);
  if (metadata === null) throw new Error(`Unknown built-in profile: ${id}`);
  return metadata.defaultProfile.name;
}

export function defaultDictationProfiles(): DictationProfile[] {
  return BUILT_IN_DICTATION_PROFILE_METADATA.map(({ defaultProfile }) =>
    structuredClone(defaultProfile),
  );
}

function builtInAltShortcut(keys: readonly ShortcutKey[]): Shortcut {
  return deepFreeze({
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys: [...keys],
  });
}

function deepFreeze<Value>(value: Value): Value {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const nested of Object.values(value)) deepFreeze(nested);
  }
  return value;
}
