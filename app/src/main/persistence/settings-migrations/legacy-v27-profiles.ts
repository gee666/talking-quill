import { z } from 'zod';

// Frozen released contract. Do not import mutable current schemas.
export const LegacyShortcutKeyV27Schema = z.enum([
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
const Modifiers = z
  .object({ ctrl: z.boolean(), alt: z.boolean(), shift: z.boolean(), meta: z.boolean() })
  .strict();
export const LegacyShortcutV27Schema = z
  .object({
    modifiers: Modifiers,
    keys: z
      .array(LegacyShortcutKeyV27Schema)
      .min(1)
      .max(26)
      .refine((keys) => new Set(keys).size === keys.length),
  })
  .strict()
  .refine(({ modifiers }) => modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.meta);
const BuiltInId = z.enum([
  'general',
  'prompt',
  'prompt-to-english',
  'markdown',
  'translate-to-english',
]);
export const ProfileId = z.union([BuiltInId, z.uuid()]);
export type LegacyShortcutV27 = z.infer<typeof LegacyShortcutV27Schema>;
type LegacyShortcut = LegacyShortcutV27;

const ReleasedBuiltInShortcuts: readonly {
  readonly id: z.infer<typeof BuiltInId>;
  readonly shortcut: LegacyShortcut;
}[] = [
  { id: 'general', shortcut: releasedAltShortcut(['X']) },
  { id: 'prompt', shortcut: releasedAltShortcut(['X', 'P']) },
  { id: 'prompt-to-english', shortcut: releasedAltShortcut(['X', 'Q']) },
  { id: 'markdown', shortcut: releasedAltShortcut(['X', 'M']) },
  { id: 'translate-to-english', shortcut: releasedAltShortcut(['X', 'T']) },
];

export const LegacyDictationProfileV27Schema = z
  .object({
    id: ProfileId,
    name: z.string().trim().min(1).max(80),
    shortcut: LegacyShortcutV27Schema,
    processingMode: z.enum(['raw', 'smart']),
    smartPrompt: z.string().trim().max(4096).nullable(),
  })
  .strict();
export const LegacyDictationProfileListV27Schema = z
  .array(LegacyDictationProfileV27Schema)
  .min(5)
  .max(13)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const shortcuts = new Set<string>();
    const priorProfiles: { readonly id: string; readonly shortcut: LegacyShortcut }[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id))
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      ids.add(profile.id);
      if (releasedReservedBindingForProfile(profile.id, profile.shortcut))
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'The default built-in profile shortcuts are reserved for their owners.',
        });
      const identity = JSON.stringify(profile.shortcut);
      if (shortcuts.has(identity))
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts must be distinct',
        });
      else if (
        priorProfiles.some(
          (candidate) =>
            releasedShortcutsConflict(candidate.shortcut, profile.shortcut) &&
            !releasedCanonicalFamilyPair(
              candidate.id,
              candidate.shortcut,
              profile.id,
              profile.shortcut,
            ),
        )
      )
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message:
            'Profile shortcuts with the same modifiers must not prefix one another outside the built-in default family',
        });
      shortcuts.add(identity);
      priorProfiles.push(profile);
    }
    for (const id of BuiltInId.options)
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
  });

function releasedAltShortcut(keys: LegacyShortcut['keys']): LegacyShortcut {
  return { modifiers: { ctrl: false, alt: true, shift: false, meta: false }, keys };
}

function releasedShortcutsEqual(left: LegacyShortcut, right: LegacyShortcut): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function releasedShortcutsConflict(left: LegacyShortcut, right: LegacyShortcut): boolean {
  if (JSON.stringify(left.modifiers) !== JSON.stringify(right.modifiers)) return false;
  const prefixLength = Math.min(left.keys.length, right.keys.length);
  return left.keys.slice(0, prefixLength).every((key, index) => key === right.keys[index]);
}

function releasedCanonicalOwner(id: string, shortcut: LegacyShortcut): boolean {
  return ReleasedBuiltInShortcuts.some(
    (candidate) => candidate.id === id && releasedShortcutsEqual(candidate.shortcut, shortcut),
  );
}

function releasedCanonicalFamilyPair(
  leftId: string,
  left: LegacyShortcut,
  rightId: string,
  right: LegacyShortcut,
): boolean {
  return (
    leftId !== rightId &&
    releasedCanonicalOwner(leftId, left) &&
    releasedCanonicalOwner(rightId, right)
  );
}

function releasedReservedBindingForProfile(id: string, shortcut: LegacyShortcut): boolean {
  if (releasedCanonicalOwner(id, shortcut)) return false;
  return ReleasedBuiltInShortcuts.some((candidate) =>
    releasedShortcutsConflict(candidate.shortcut, shortcut),
  );
}
