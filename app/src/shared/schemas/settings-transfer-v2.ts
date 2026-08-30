import { z } from 'zod';
import { LegacyTransferV1ProfileSchema } from './settings-transfer-v1';
import type { LegacyTransferV1ShortcutSchema } from './settings-transfer-v1';

// Frozen snapshot of the dictation-profile transfer v2 contract. V2 deliberately permits
// shared-prefix shortcuts while retaining the v1 A-Z wire shape. It may depend only on the frozen
// v1 field schemas, never on mutable current profile or shortcut schemas.
export const LegacyTransferV2ProfileSchema = LegacyTransferV1ProfileSchema;

type LegacyTransferV2Shortcut = z.infer<typeof LegacyTransferV1ShortcutSchema>;
type LegacyTransferV2Profile = z.infer<typeof LegacyTransferV2ProfileSchema>;

const LEGACY_TRANSFER_V2_BUILT_IN_SHORTCUTS: readonly {
  readonly id: LegacyTransferV2Profile['id'];
  readonly shortcut: LegacyTransferV2Shortcut;
}[] = [
  { id: 'general', shortcut: legacyAltShortcut(['X']) },
  { id: 'prompt', shortcut: legacyAltShortcut(['X', 'P']) },
  { id: 'prompt-to-english', shortcut: legacyAltShortcut(['X', 'Q']) },
  { id: 'markdown', shortcut: legacyAltShortcut(['X', 'M']) },
  { id: 'translate-to-english', shortcut: legacyAltShortcut(['X', 'T']) },
];

const LEGACY_TRANSFER_V2_BUILT_IN_IDS = [
  'general',
  'prompt',
  'prompt-to-english',
  'markdown',
  'translate-to-english',
] as const;

export const LegacyTransferV2ProfileListSchema = z
  .array(LegacyTransferV2ProfileSchema)
  .min(LEGACY_TRANSFER_V2_BUILT_IN_IDS.length)
  .max(13)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const shortcuts = new Set<string>();
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
      }
      shortcuts.add(identity);
    }
    for (const id of LEGACY_TRANSFER_V2_BUILT_IN_IDS) {
      if (!ids.has(id)) {
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
      }
    }
  });

function legacyAltShortcut(keys: LegacyTransferV2Shortcut['keys']): LegacyTransferV2Shortcut {
  return {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys,
  };
}

function legacyShortcutsEqual(
  left: LegacyTransferV2Shortcut,
  right: LegacyTransferV2Shortcut,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function legacyReservedBindingForProfile(id: string, shortcut: LegacyTransferV2Shortcut): boolean {
  const owner = LEGACY_TRANSFER_V2_BUILT_IN_SHORTCUTS.find(({ shortcut: candidate }) =>
    legacyShortcutsEqual(candidate, shortcut),
  )?.id;
  return owner !== undefined && owner !== id;
}
