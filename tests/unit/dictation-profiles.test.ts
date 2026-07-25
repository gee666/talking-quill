import { describe, expect, it } from 'vitest';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
} from '../../app/src/shared/schemas/dictation-profiles';

const defaults = () => [
  structuredClone(DEFAULT_GENERAL_PROFILE),
  structuredClone(DEFAULT_PROMPT_PROFILE),
];

describe('dictation profile contracts', () => {
  it('defines the complete literal built-in defaults', () => {
    expect(DEFAULT_GENERAL_PROFILE).toEqual({
      id: 'general',
      name: 'General',
      activationKey: 'Z',
      shift: false,
      processingMode: 'raw',
      smartPrompt: null,
    });
    expect(DEFAULT_PROMPT_PROFILE).toEqual({
      id: 'prompt',
      name: 'Prompt',
      activationKey: 'Z',
      shift: true,
      processingMode: 'smart',
      smartPrompt:
        'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
    });
    expect(DictationProfileListSchema.safeParse(defaults()).success).toBe(true);
  });

  it('rejects duplicate exact bindings while allowing the same letter with different Shift state', () => {
    const valid = defaults();
    valid.push({
      id: '11111111-1111-4111-8111-111111111111',
      name: 'Custom',
      activationKey: 'A',
      shift: false,
      processingMode: 'smart',
      smartPrompt: 'Use short paragraphs.',
    });
    expect(DictationProfileListSchema.safeParse(valid).success).toBe(true);
    const custom = valid[2];
    if (custom === undefined) throw new Error('Custom profile is missing');
    valid[2] = { ...custom, activationKey: 'Z' };
    expect(DictationProfileListSchema.safeParse(valid).success).toBe(false);
  });

  it('reserves both built-in default chords even after their owners move away', () => {
    const moved = [
      { ...structuredClone(DEFAULT_GENERAL_PROFILE), activationKey: 'G' as const },
      { ...structuredClone(DEFAULT_PROMPT_PROFILE), activationKey: 'P' as const },
    ];
    for (const shift of [false, true]) {
      expect(
        DictationProfileListSchema.safeParse([
          ...moved,
          {
            id: '11111111-1111-4111-8111-111111111111',
            name: 'Custom',
            activationKey: 'Z',
            shift,
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
        activationKey: 'Z',
        shift: true,
        processingMode: 'raw',
        smartPrompt: null,
      }).success,
    ).toBe(false);
  });

  it('requires complete binding pairs while allowing strict nonbinding patches', () => {
    expect(DictationProfilePatchSchema.safeParse({ activationKey: 'Q' }).success).toBe(false);
    expect(DictationProfilePatchSchema.safeParse({ shift: true }).success).toBe(false);
    expect(DictationProfilePatchSchema.safeParse({}).success).toBe(false);
    expect(DictationProfilePatchSchema.safeParse({ activationKey: 'Q', shift: true }).success).toBe(
      true,
    );
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
