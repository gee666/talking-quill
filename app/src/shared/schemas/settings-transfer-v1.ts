import { z } from 'zod';

// Frozen snapshot of the dictation-profile transfer v1 contract. Keep this file independent of
// current profile and shortcut schemas so future validation changes cannot reinterpret v1 files.
const LegacyTransferV1LetterSchema = z.enum([
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

export const LegacyTransferV1ShortcutSchema = z
  .object({
    modifiers: z
      .object({
        ctrl: z.boolean(),
        alt: z.boolean(),
        shift: z.boolean(),
        meta: z.boolean(),
      })
      .strict(),
    keys: z
      .array(LegacyTransferV1LetterSchema)
      .min(1)
      .max(26)
      .refine((keys) => new Set(keys).size === keys.length),
  })
  .strict()
  .refine(
    ({ modifiers }) => modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.meta,
    'At least one modifier is required',
  );

const LegacyTransferV1BuiltInIdSchema = z.enum([
  'general',
  'prompt',
  'prompt-to-english',
  'markdown',
  'translate-to-english',
]);

export const LegacyTransferV1ProfileSchema = z
  .object({
    id: z.union([LegacyTransferV1BuiltInIdSchema, z.uuid()]),
    name: z.string().trim().min(1).max(80),
    shortcut: LegacyTransferV1ShortcutSchema,
    processingMode: z.enum(['raw', 'smart']),
    smartPrompt: z.string().trim().max(4_096).nullable(),
  })
  .strict();

type LegacyTransferV1Shortcut = z.infer<typeof LegacyTransferV1ShortcutSchema>;

const LEGACY_TRANSFER_V1_BUILT_IN_SHORTCUTS: readonly {
  readonly id: z.infer<typeof LegacyTransferV1BuiltInIdSchema>;
  readonly shortcut: LegacyTransferV1Shortcut;
}[] = [
  { id: 'general', shortcut: legacyAltShortcut(['X']) },
  { id: 'prompt', shortcut: legacyAltShortcut(['X', 'P']) },
  { id: 'prompt-to-english', shortcut: legacyAltShortcut(['X', 'Q']) },
  { id: 'markdown', shortcut: legacyAltShortcut(['X', 'M']) },
  { id: 'translate-to-english', shortcut: legacyAltShortcut(['X', 'T']) },
];

export const LegacyTransferV1ProfileListSchema = z
  .array(LegacyTransferV1ProfileSchema)
  .min(5)
  .max(13)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const shortcuts = new Set<string>();
    const priorProfiles: {
      readonly id: string;
      readonly shortcut: LegacyTransferV1Shortcut;
    }[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      }
      ids.add(profile.id);
      if (legacyReservedBindingForProfile(profile.id, profile.shortcut)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'The default built-in profile shortcuts are reserved for their owners.',
        });
      }
      const identity = JSON.stringify(profile.shortcut);
      if (shortcuts.has(identity)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts must be distinct',
        });
      } else if (
        priorProfiles.some(
          (candidate) =>
            legacyShortcutsConflict(candidate.shortcut, profile.shortcut) &&
            !legacyCanonicalFamilyPair(
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
      shortcuts.add(identity);
      priorProfiles.push(profile);
    }
    for (const id of LegacyTransferV1BuiltInIdSchema.options) {
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
    }
  });

export type LegacyTransferV1Profile = z.infer<typeof LegacyTransferV1ProfileSchema>;

function legacyAltShortcut(keys: LegacyTransferV1Shortcut['keys']): LegacyTransferV1Shortcut {
  return {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys,
  };
}

function legacyShortcutsEqual(
  left: LegacyTransferV1Shortcut,
  right: LegacyTransferV1Shortcut,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function legacyShortcutsConflict(
  left: LegacyTransferV1Shortcut,
  right: LegacyTransferV1Shortcut,
): boolean {
  if (JSON.stringify(left.modifiers) !== JSON.stringify(right.modifiers)) return false;
  const prefixLength = Math.min(left.keys.length, right.keys.length);
  return left.keys.slice(0, prefixLength).every((key, index) => key === right.keys[index]);
}

function legacyCanonicalOwner(id: string, shortcut: LegacyTransferV1Shortcut): boolean {
  return LEGACY_TRANSFER_V1_BUILT_IN_SHORTCUTS.some(
    (candidate) => candidate.id === id && legacyShortcutsEqual(candidate.shortcut, shortcut),
  );
}

function legacyCanonicalFamilyPair(
  leftId: string,
  left: LegacyTransferV1Shortcut,
  rightId: string,
  right: LegacyTransferV1Shortcut,
): boolean {
  return (
    leftId !== rightId && legacyCanonicalOwner(leftId, left) && legacyCanonicalOwner(rightId, right)
  );
}

function legacyReservedBindingForProfile(id: string, shortcut: LegacyTransferV1Shortcut): boolean {
  if (legacyCanonicalOwner(id, shortcut)) return false;
  return LEGACY_TRANSFER_V1_BUILT_IN_SHORTCUTS.some((candidate) =>
    legacyShortcutsConflict(candidate.shortcut, shortcut),
  );
}
