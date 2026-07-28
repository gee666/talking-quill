import { describe, expect, it } from 'vitest';
import {
  BUILT_IN_DICTATION_PROFILE_METADATA,
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
  MAX_DICTATION_PROFILES,
  defaultDictationProfiles,
} from '../../app/src/shared/schemas/dictation-profiles';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import type { Shortcut } from '../../app/src/shared/schemas/shortcut';

const defaults = () => defaultDictationProfiles();
const shortcut = (
  keys: Shortcut['keys'],
  modifiers: Partial<Shortcut['modifiers']> = {},
): Shortcut => ({
  modifiers: { ctrl: false, alt: true, shift: false, meta: false, ...modifiers },
  keys,
});

describe('dictation profile contracts', () => {
  it('defines complete centralized, frozen built-in defaults', () => {
    expect(BUILT_IN_DICTATION_PROFILE_METADATA.map(({ id }) => id)).toEqual([
      'general',
      'prompt',
      'prompt-to-english',
      'markdown',
      'translate-to-english',
    ]);
    expect(DEFAULT_GENERAL_PROFILE).toEqual({
      id: 'general',
      name: 'General',
      shortcut: shortcut(['X']),
      processingMode: 'smart',
      smartPrompt: 'Clean up and format the transcript while preserving its source language.',
    });
    expect(DEFAULT_PROMPT_PROFILE).toMatchObject({
      id: 'prompt',
      name: 'Prompt',
      shortcut: shortcut(['X', 'P']),
      processingMode: 'smart',
    });
    expect(DEFAULT_PROMPT_PROFILE.smartPrompt).toContain('Preserve the source language.');
    expect(DEFAULT_PROMPT_PROFILE.smartPrompt).toContain('clear paragraphs and lists');
    expect(DEFAULT_PROMPT_TO_ENGLISH_PROFILE).toMatchObject({
      id: 'prompt-to-english',
      name: 'Prompt to English',
      shortcut: shortcut(['X', 'Q']),
      processingMode: 'smart',
    });
    expect(DEFAULT_PROMPT_TO_ENGLISH_PROFILE.smartPrompt).toContain(
      'Translate the result to natural English',
    );
    expect(DEFAULT_MARKDOWN_PROFILE).toEqual({
      id: 'markdown',
      name: 'Markdown',
      shortcut: shortcut(['X', 'M']),
      processingMode: 'smart',
      smartPrompt:
        'Format the transcript as clear Markdown using headings, paragraphs, and lists where useful. Preserve its source language.',
    });
    expect(DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE).toEqual({
      id: 'translate-to-english',
      name: 'Translate to English',
      shortcut: shortcut(['X', 'T']),
      processingMode: 'smart',
      smartPrompt:
        'Translate the transcript to natural English while preserving its meaning, tone, facts, names, numbers, and level of detail.',
    });
    for (const { defaultProfile } of BUILT_IN_DICTATION_PROFILE_METADATA) {
      expect(Object.isFrozen(defaultProfile)).toBe(true);
      expect(Object.isFrozen(defaultProfile.shortcut.modifiers)).toBe(true);
      expect(Object.isFrozen(defaultProfile.shortcut.keys)).toBe(true);
    }
    expect(DictationProfileListSchema.safeParse(defaults()).success).toBe(true);
    expect(DEFAULT_SETTINGS.app.defaultProcessingMode).toBe('smart');
    expect(DEFAULT_SETTINGS.transcription.language).toBe('auto');
  });

  it('accepts thirteen total profiles and rejects fourteen', () => {
    const profiles = defaults();
    for (let index = 0; index < 8; index += 1) {
      profiles.push({
        id: `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`,
        name: `Custom ${String(index)}`,
        shortcut: shortcut([String.fromCharCode(65 + index) as Shortcut['keys'][number]], {
          ctrl: true,
          alt: false,
        }),
        processingMode: 'raw',
        smartPrompt: null,
      });
    }
    expect(profiles).toHaveLength(MAX_DICTATION_PROFILES);
    expect(DictationProfileListSchema.safeParse(profiles).success).toBe(true);
    expect(
      DictationProfileListSchema.safeParse([
        ...profiles,
        {
          id: '00000000-0000-4000-8000-000000000099',
          name: 'Too many',
          shortcut: shortcut(['Y'], { ctrl: true, alt: false }),
          processingMode: 'raw',
          smartPrompt: null,
        },
      ]).success,
    ).toBe(false);
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
    const custom = valid[5];
    const other = valid[6];
    if (custom === undefined || other === undefined) throw new Error('Custom profiles are missing');
    valid[6] = { ...other, shortcut: structuredClone(custom.shortcut) };
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

  it('reserves every built-in default chord prefix even after its owner moves away', () => {
    const moved = defaults().map((profile, index) => ({
      ...profile,
      shortcut: shortcut([String.fromCharCode(71 + index) as Shortcut['keys'][number]], {
        ctrl: true,
        alt: false,
      }),
    }));
    for (const reservedShortcut of [
      shortcut(['X']),
      shortcut(['X', 'P']),
      shortcut(['X', 'P', 'E']),
      shortcut(['X', 'M']),
      shortcut(['X', 'E']),
      shortcut(['X', 'Q']),
      shortcut(['X', 'T']),
    ]) {
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
    const editedGeneral = defaults();
    const general = editedGeneral[0];
    if (general === undefined) throw new Error('General profile is missing');
    editedGeneral[0] = { ...general, shortcut: shortcut(['X', 'Q']) };
    expect(DictationProfileListSchema.safeParse(editedGeneral).success).toBe(false);

    const renamedGeneral = defaults();
    const canonicalGeneral = renamedGeneral[0];
    if (canonicalGeneral === undefined) throw new Error('General profile is missing');
    renamedGeneral[0] = { ...canonicalGeneral, name: 'Renamed General' };
    expect(DictationProfileListSchema.safeParse(renamedGeneral).success).toBe(true);

    expect(DictationProfileListSchema.safeParse(defaults()).success).toBe(true);
    expect(
      DictationProfileCreateSchema.safeParse({
        name: 'Reserved custom',
        shortcut: shortcut(['X', 'E']),
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

  it('requires all nondeletable built-in identities', () => {
    for (const profile of defaults()) {
      expect(
        DictationProfileListSchema.safeParse(defaults().filter(({ id }) => id !== profile.id))
          .success,
      ).toBe(false);
    }
  });
});
