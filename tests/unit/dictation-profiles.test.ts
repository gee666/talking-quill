import { describe, expect, it } from 'vitest';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
} from '../../app/src/shared/schemas/dictation-profiles';
import { shortcutFromLegacyActivation, type Shortcut } from '../../app/src/shared/schemas/shortcut';

const defaults = () => [
  structuredClone(DEFAULT_GENERAL_PROFILE),
  structuredClone(DEFAULT_PROMPT_PROFILE),
];
const shortcut = (
  keys: Shortcut['keys'],
  modifiers: Partial<Shortcut['modifiers']> = {},
): Shortcut => ({
  modifiers: { ctrl: false, alt: true, shift: false, meta: false, ...modifiers },
  keys,
});

describe('dictation profile contracts', () => {
  it('defines the complete literal built-in defaults', () => {
    expect(DEFAULT_GENERAL_PROFILE).toEqual({
      id: 'general',
      name: 'General',
      shortcut: shortcutFromLegacyActivation('Z', false),
      processingMode: 'raw',
      smartPrompt: null,
    });
    expect(DEFAULT_PROMPT_PROFILE).toEqual({
      id: 'prompt',
      name: 'Prompt',
      shortcut: shortcutFromLegacyActivation('Z', true),
      processingMode: 'smart',
      smartPrompt:
        'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
    });
    expect(Object.isFrozen(DEFAULT_GENERAL_PROFILE.shortcut.modifiers)).toBe(true);
    expect(Object.isFrozen(DEFAULT_GENERAL_PROFILE.shortcut.keys)).toBe(true);
    expect(DictationProfileListSchema.safeParse(defaults()).success).toBe(true);
  });

  it('rejects duplicate full identities while allowing different modifiers and order', () => {
    const valid = defaults();
    valid.push({
      id: '11111111-1111-4111-8111-111111111111',
      name: 'Custom',
      shortcut: shortcut(['A']),
      processingMode: 'smart',
      smartPrompt: 'Use short paragraphs.',
    });
    valid.push({
      id: '22222222-2222-4222-8222-222222222222',
      name: 'Other',
      shortcut: shortcut(['A'], { ctrl: true }),
      processingMode: 'raw',
      smartPrompt: null,
    });
    valid.push({
      id: '33333333-3333-4333-8333-333333333333',
      name: 'Ordered',
      shortcut: shortcut(['B', 'A']),
      processingMode: 'raw',
      smartPrompt: null,
    });
    expect(DictationProfileListSchema.safeParse(valid).success).toBe(true);
    const custom = valid[2];
    if (custom === undefined) throw new Error('Custom profile is missing');
    const other = valid[3];
    if (other === undefined) throw new Error('Other profile is missing');
    valid[3] = { ...other, shortcut: structuredClone(custom.shortcut) };
    expect(DictationProfileListSchema.safeParse(valid).success).toBe(false);
  });

  it('rejects same-modifier prefix conflicts but allows different modifiers', () => {
    const profiles = [
      ...defaults(),
      {
        id: '11111111-1111-4111-8111-111111111111',
        name: 'Short',
        shortcut: shortcut(['A']),
        processingMode: 'raw' as const,
        smartPrompt: null,
      },
    ];
    expect(
      DictationProfileListSchema.safeParse([
        ...profiles,
        {
          id: '22222222-2222-4222-8222-222222222222',
          name: 'Long',
          shortcut: shortcut(['A', 'B']),
          processingMode: 'raw',
          smartPrompt: null,
        },
      ]).success,
    ).toBe(false);
    expect(
      DictationProfileListSchema.safeParse([
        ...profiles,
        {
          id: '22222222-2222-4222-8222-222222222222',
          name: 'Long shifted',
          shortcut: shortcut(['A', 'B'], { shift: true }),
          processingMode: 'raw',
          smartPrompt: null,
        },
      ]).success,
    ).toBe(true);
  });

  it('reserves both built-in default chord prefixes even after their owners move away', () => {
    const moved = [
      { ...structuredClone(DEFAULT_GENERAL_PROFILE), shortcut: shortcut(['G']) },
      { ...structuredClone(DEFAULT_PROMPT_PROFILE), shortcut: shortcut(['P'], { shift: true }) },
    ];
    for (const reservedShortcut of [shortcut(['Z', 'A']), shortcut(['Z', 'A'], { shift: true })]) {
      expect(
        DictationProfileListSchema.safeParse([
          ...moved,
          {
            id: '11111111-1111-4111-8111-111111111111',
            name: 'Custom',
            shortcut: reservedShortcut,
            processingMode: 'raw',
            smartPrompt: null,
          },
        ]).success,
      ).toBe(false);
    }
    expect(DictationProfileListSchema.safeParse(defaults()).success).toBe(true);
    expect(
      DictationProfileCreateSchema.safeParse({
        name: 'Reserved custom',
        shortcut: shortcut(['Z'], { shift: true }),
        processingMode: 'raw',
        smartPrompt: null,
      }).success,
    ).toBe(false);
  });

  it('requires a complete shortcut while allowing strict nonbinding patches', () => {
    expect(DictationProfilePatchSchema.safeParse({ shortcut: { keys: ['Q'] } }).success).toBe(
      false,
    );
    expect(DictationProfilePatchSchema.safeParse({}).success).toBe(false);
    expect(
      DictationProfilePatchSchema.safeParse({ shortcut: shortcut(['Q'], { shift: true }) }).success,
    ).toBe(true);
    expect(
      DictationProfilePatchSchema.safeParse({ name: 'Renamed', processingMode: 'smart' }).success,
    ).toBe(true);
    expect(DictationProfilePatchSchema.safeParse({ name: 'Renamed', unknown: true }).success).toBe(
      false,
    );
  });

  it('requires both nondeletable built-in identities', () => {
    expect(DictationProfileListSchema.safeParse([DEFAULT_GENERAL_PROFILE]).success).toBe(false);
  });
});
