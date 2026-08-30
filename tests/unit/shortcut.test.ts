import { describe, expect, it } from 'vitest';
import { shortcutPlatformPolicy } from '../../app/src/shared/schemas/shortcut-platform-policy';
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
  it('accepts ordered unique A-Z sequences at both 1-key and 26-key bounds', () => {
    const value = shortcut(['Q', 'A'], { ctrl: true, shift: true, meta: true });
    expect(ShortcutSchema.parse(value)).toEqual(value);
    expect(shortcutTrigger(value)).toBe('A');
    expect(ShortcutSchema.parse(shortcut([...ShortcutKeySchema.options])).keys).toEqual(
      ShortcutKeySchema.options,
    );
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

  it('requires at least one modifier while allowing all 15 nonempty modifier aggregates', () => {
    expect(
      ShortcutSchema.safeParse(
        shortcut(['A'], { ctrl: false, alt: false, shift: false, meta: false }),
      ).success,
    ).toBe(false);

    const names = ['ctrl', 'alt', 'shift', 'meta'] as const;
    for (let mask = 1; mask < 16; mask += 1) {
      const modifiers = Object.fromEntries(
        names.map((name, index) => [name, (mask & (1 << index)) !== 0]),
      ) as Shortcut['modifiers'];
      expect(ShortcutSchema.safeParse({ modifiers, keys: ['A'] }).success, String(mask)).toBe(true);
    }
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

  it('identifies equal and shared-prefix sequences without treating them as the same shortcut', () => {
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A']))).toBe(true);
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A', 'B']))).toBe(true);
    expect(shortcutsEqual(shortcut(['A']), shortcut(['A', 'B']))).toBe(false);
    expect(shortcutsConflict(shortcut(['A', 'B']), shortcut(['A']))).toBe(true);
    expect(shortcutsConflict(shortcut(['A', 'B']), shortcut(['A', 'C']))).toBe(false);
    expect(shortcutsConflict(shortcut(['A']), shortcut(['A', 'B'], { shift: true }))).toBe(false);
  });

  it('classifies exact impossible and risky platform combinations without restricting storage', () => {
    const decisions = {
      windowsLock: shortcutPlatformPolicy(
        shortcut(['L'], { ctrl: false, alt: false, shift: false, meta: true }),
        'win32',
      ),
      windowsLockPrefix: shortcutPlatformPolicy(
        shortcut(['L', 'A'], { ctrl: false, alt: false, shift: false, meta: true }),
        'win32',
      ),
      windowsLockSuffix: shortcutPlatformPolicy(
        shortcut(['A', 'L'], { ctrl: false, alt: false, shift: false, meta: true }),
        'win32',
      ),
      windowsAltGr: shortcutPlatformPolicy(
        shortcut(['A'], { ctrl: true, alt: true, shift: false, meta: false }),
        'win32',
      ),
      macLock: shortcutPlatformPolicy(
        shortcut(['Q'], { ctrl: true, alt: false, shift: false, meta: true }),
        'darwin',
      ),
      macLockSuffix: shortcutPlatformPolicy(
        shortcut(['A', 'Q'], { ctrl: true, alt: false, shift: false, meta: true }),
        'darwin',
      ),
      shiftTyping: shortcutPlatformPolicy(
        shortcut(['A'], { ctrl: false, alt: false, shift: true, meta: false }),
        'linux',
      ),
      commonControl: shortcutPlatformPolicy(
        shortcut(['B'], { ctrl: true, alt: false, shift: false, meta: false }),
        'win32',
      ),
      commonControlShift: shortcutPlatformPolicy(
        shortcut(['T'], { ctrl: true, alt: false, shift: true, meta: false }),
        'win32',
      ),
      allowedAggregate: shortcutPlatformPolicy(
        shortcut(['A'], { ctrl: true, alt: false, shift: true, meta: true }),
        'linux',
      ),
    };

    expect(decisions).toMatchInlineSnapshot(`
      {
        "allowedAggregate": {
          "status": "allowed",
        },
        "commonControl": {
          "code": "widely-used-editing-command",
          "message": "This is a widely used application shortcut family. Talking Quill captures it globally while it is enabled.",
          "status": "risky",
        },
        "commonControlShift": {
          "code": "widely-used-editing-command",
          "message": "This is a widely used application shortcut family. Talking Quill captures it globally while it is enabled.",
          "status": "risky",
        },
        "macLock": {
          "code": "macos-lock-screen",
          "message": "macOS reserves Control + Command + Q for locking the screen, so Talking Quill cannot receive this shortcut reliably.",
          "status": "impossible",
        },
        "macLockSuffix": {
          "code": "macos-lock-screen",
          "message": "macOS reserves Control + Command + Q for locking the screen, so Talking Quill cannot receive this shortcut reliably.",
          "status": "impossible",
        },
        "shiftTyping": {
          "code": "ordinary-shift-typing",
          "message": "A Shift-only shortcut can capture ordinary capital-letter typing in every app while Talking Quill is enabled.",
          "status": "risky",
        },
        "windowsAltGr": {
          "code": "windows-altgr",
          "message": "Ctrl + Alt shortcuts can overlap AltGr typing on some keyboard layouts. Talking Quill distinguishes physical AltGr, but this combination may still be surprising.",
          "status": "risky",
        },
        "windowsLock": {
          "code": "windows-lock-screen",
          "message": "Windows reserves Win + L for locking the computer, so Talking Quill cannot receive this shortcut.",
          "status": "impossible",
        },
        "windowsLockPrefix": {
          "code": "windows-lock-screen",
          "message": "Windows reserves Win + L for locking the computer, so Talking Quill cannot receive this shortcut.",
          "status": "impossible",
        },
        "windowsLockSuffix": {
          "code": "windows-lock-screen",
          "message": "Windows reserves Win + L for locking the computer, so Talking Quill cannot receive this shortcut.",
          "status": "impossible",
        },
      }
    `);
  });
});
