import { describe, expect, it } from 'vitest';
import {
  ShortcutKeySchema,
  ShortcutSchema,
  shortcutFromLegacyActivation,
  shortcutIdentity,
  shortcutTrigger,
  shortcutsConflict,
  shortcutsEqual,
  type Shortcut,
} from '../../app/src/shared/schemas/shortcut';

const shortcut = (
  keys: Shortcut['keys'],
  modifiers: Partial<Shortcut['modifiers']> = {},
): Shortcut => ({
  modifiers: { ctrl: false, alt: true, shift: false, meta: false, ...modifiers },
  keys,
});

describe('shortcut contracts', () => {
  it('accepts every explicit modifier and ordered unique A-Z key sequence', () => {
    const value = shortcut(['Q', 'A'], { ctrl: true, shift: true, meta: true });
    expect(ShortcutSchema.parse(value)).toEqual(value);
    expect(shortcutTrigger(value)).toBe('A');
  });

  it('rejects empty, duplicate, lowercase, and overlong key sequences', () => {
    expect(ShortcutSchema.safeParse(shortcut([])).success).toBe(false);
    expect(ShortcutSchema.safeParse(shortcut(['A', 'A'])).success).toBe(false);
    expect(ShortcutSchema.safeParse({ ...shortcut(['A']), keys: ['a'] }).success).toBe(false);
    expect(
      ShortcutSchema.safeParse({
        ...shortcut(['A']),
        keys: [...ShortcutKeySchema.options, 'A'],
      }).success,
    ).toBe(false);
  });

  it('requires at least one modifier while allowing every nonempty combination', () => {
    expect(
      ShortcutSchema.safeParse(
        shortcut(['A'], { ctrl: false, alt: false, shift: false, meta: false }),
      ).success,
    ).toBe(false);
    expect(
      ShortcutSchema.safeParse(
        shortcut(['A'], { ctrl: false, alt: false, shift: true, meta: false }),
      ).success,
    ).toBe(true);
    expect(
      ShortcutSchema.safeParse(
        shortcut(['A'], { ctrl: false, alt: false, shift: false, meta: true }),
      ).success,
    ).toBe(true);
  });

  it('requires the complete strict modifier shape', () => {
    expect(
      ShortcutSchema.safeParse({ modifiers: { alt: true, shift: false }, keys: ['A'] }).success,
    ).toBe(false);
    expect(
      ShortcutSchema.safeParse({
        ...shortcut(['A']),
        modifiers: { ...shortcut(['A']).modifiers, capsLock: false },
      }).success,
    ).toBe(false);
  });

  it('maps legacy bindings to Alt chords without losing Shift', () => {
    expect(shortcutFromLegacyActivation('Z', false)).toEqual(shortcut(['Z']));
    expect(shortcutFromLegacyActivation('Z', true)).toEqual(shortcut(['Z'], { shift: true }));
  });

  it('uses every modifier and ordered key for full identity', () => {
    const base = shortcut(['A', 'B']);
    expect(shortcutsEqual(base, structuredClone(base))).toBe(true);
    expect(shortcutsEqual(base, shortcut(['B', 'A']))).toBe(false);
    expect(shortcutsEqual(base, shortcut(['A', 'B'], { meta: true }))).toBe(false);
    expect(shortcutIdentity(base)).not.toBe(shortcutIdentity(shortcut(['A', 'B'], { ctrl: true })));
  });

  it('conflicts only for equal modifiers and equal-or-prefix key sequences', () => {
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A']))).toBe(true);
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A', 'B']))).toBe(true);
    expect(shortcutsConflict(shortcut(['A', 'B']), shortcut(['A']))).toBe(true);
    expect(shortcutsConflict(shortcut(['A', 'B']), shortcut(['A', 'C']))).toBe(false);
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A', 'B'], { shift: true }))).toBe(false);
  });
});
