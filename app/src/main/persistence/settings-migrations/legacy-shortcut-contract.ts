import {
  LegacyShortcutKeyV27Schema,
  type LegacyShortcutV27 as Shortcut,
} from './legacy-settings-v27';

export const GENERAL_PROFILE_ID = 'general' as const;
export const PROMPT_PROFILE_ID = 'prompt' as const;

interface MigrationProfile {
  readonly id: 'general' | 'prompt' | 'prompt-to-english' | 'markdown' | 'translate-to-english';
  readonly name: string;
  readonly shortcut: Shortcut;
  readonly processingMode: 'raw' | 'smart';
  readonly smartPrompt: string | null;
}

export const DEFAULT_GENERAL_PROFILE: MigrationProfile = {
  id: 'general',
  name: 'General',
  shortcut: altShortcut(['X']),
  processingMode: 'smart',
  smartPrompt: 'Clean up and format the transcript while preserving its source language.',
};

export const DEFAULT_PROMPT_PROFILE: MigrationProfile = {
  id: 'prompt',
  name: 'Prompt',
  shortcut: altShortcut(['X', 'P']),
  processingMode: 'smart',
  smartPrompt:
    'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Preserve the source language. Organize the result into clear paragraphs and lists when useful, and use tables or other formatting when helpful.',
};

export const DEFAULT_PROMPT_TO_ENGLISH_PROFILE: MigrationProfile = {
  id: 'prompt-to-english',
  name: 'Prompt to English',
  shortcut: altShortcut(['X', 'Q']),
  processingMode: 'smart',
  smartPrompt:
    'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Translate the result to natural English while preserving its meaning, tone, facts, names, numbers, and level of detail. Organize the result into clear paragraphs and lists when useful, and use tables or other formatting when helpful.',
};

export const DEFAULT_MARKDOWN_PROFILE: MigrationProfile = {
  id: 'markdown',
  name: 'Markdown',
  shortcut: altShortcut(['X', 'M']),
  processingMode: 'smart',
  smartPrompt:
    'Format the transcript as clear Markdown using headings, paragraphs, and lists where useful. Preserve its source language.',
};

export const DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE: MigrationProfile = {
  id: 'translate-to-english',
  name: 'Translate to English',
  shortcut: altShortcut(['X', 'T']),
  processingMode: 'smart',
  smartPrompt:
    'Translate the transcript to natural English while preserving its meaning, tone, facts, names, numbers, and level of detail.',
};

const DEFAULT_PROFILES = [
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
] as const;

export const ShortcutKeySchema = LegacyShortcutKeyV27Schema;
export type { Shortcut };

export function defaultDictationProfiles(): MigrationProfile[] {
  return DEFAULT_PROFILES.map((profile) => structuredClone(profile));
}

export function shortcutFromLegacyActivation(
  key: Shortcut['keys'][number],
  shift: boolean,
): Shortcut {
  return {
    modifiers: { ctrl: false, alt: true, shift, meta: false },
    keys: [key],
  };
}

export function shortcutsEqual(left: Shortcut, right: Shortcut): boolean {
  return shortcutIdentity(left) === shortcutIdentity(right);
}

export function shortcutsConflict(left: Shortcut, right: Shortcut): boolean {
  if (JSON.stringify(left.modifiers) !== JSON.stringify(right.modifiers)) return false;
  const prefixLength = Math.min(left.keys.length, right.keys.length);
  return left.keys.slice(0, prefixLength).every((key, index) => key === right.keys[index]);
}

export function isReservedBindingForProfile(id: string, shortcut: Shortcut): boolean {
  if (
    DEFAULT_PROFILES.some(
      (profile) => profile.id === id && shortcutsEqual(profile.shortcut, shortcut),
    )
  ) {
    return false;
  }
  return DEFAULT_PROFILES.some((profile) => shortcutsConflict(profile.shortcut, shortcut));
}

function altShortcut(keys: Shortcut['keys']): Shortcut {
  return {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys,
  };
}

function shortcutIdentity(shortcut: Shortcut): string {
  const { ctrl, alt, shift, meta } = shortcut.modifiers;
  return `${ctrl ? '1' : '0'}${alt ? '1' : '0'}${shift ? '1' : '0'}${meta ? '1' : '0'}:${shortcut.keys.join('')}`;
}
