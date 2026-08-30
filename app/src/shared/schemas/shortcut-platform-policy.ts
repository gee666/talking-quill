import { shortcutModifiersEqual, type Shortcut, type ShortcutModifiers } from './shortcut';

export type ShortcutPlatformPolicy =
  | { readonly status: 'allowed' }
  | {
      readonly status: 'impossible' | 'risky';
      readonly code:
        | 'macos-lock-screen'
        | 'macos-option-text'
        | 'macos-system-or-app-command'
        | 'ordinary-shift-typing'
        | 'windows-alt-menu'
        | 'windows-altgr'
        | 'windows-lock-screen'
        | 'windows-system-command'
        | 'widely-used-editing-command';
      readonly message: string;
    };

const NO_MODIFIERS: ShortcutModifiers = Object.freeze({
  ctrl: false,
  alt: false,
  shift: false,
  meta: false,
});
const SHIFT_ONLY: ShortcutModifiers = Object.freeze({ ...NO_MODIFIERS, shift: true });
const META_ONLY: ShortcutModifiers = Object.freeze({ ...NO_MODIFIERS, meta: true });
const CONTROL_COMMAND: ShortcutModifiers = Object.freeze({
  ctrl: true,
  alt: false,
  shift: false,
  meta: true,
});

/**
 * Classifies platform delivery independently from the portable persisted shortcut schema.
 * Persisted settings remain portable; the editor uses this policy to reject combinations the
 * active OS consumes and to explain combinations which are interceptable but surprising.
 */
export function shortcutPlatformPolicy(
  shortcut: Shortcut,
  platform: string,
): ShortcutPlatformPolicy {
  const firstKey = shortcut.keys[0];
  if (platform === 'win32') {
    if (shortcutModifiersEqual(shortcut.modifiers, META_ONLY) && shortcut.keys.includes('L')) {
      return {
        status: 'impossible',
        code: 'windows-lock-screen',
        message:
          'Windows reserves Win + L for locking the computer, so Talking Quill cannot receive this shortcut.',
      };
    }
    if (shortcut.modifiers.ctrl && shortcut.modifiers.alt) {
      return {
        status: 'risky',
        code: 'windows-altgr',
        message:
          'Ctrl + Alt shortcuts can overlap AltGr typing on some keyboard layouts. Talking Quill distinguishes physical AltGr, but this combination may still be surprising.',
      };
    }
    if (shortcut.modifiers.meta) {
      return {
        status: 'risky',
        code: 'windows-system-command',
        message:
          'Shortcuts using the Windows key can overlap Windows commands and may be captured by the system or another app first.',
      };
    }
    if (shortcut.modifiers.alt) {
      return {
        status: 'risky',
        code: 'windows-alt-menu',
        message:
          'Alt shortcuts can overlap application menu access keys. Talking Quill captures this shortcut globally while it is enabled.',
      };
    }
  }

  if (platform === 'darwin') {
    if (
      shortcutModifiersEqual(shortcut.modifiers, CONTROL_COMMAND) &&
      shortcut.keys.includes('Q')
    ) {
      return {
        status: 'impossible',
        code: 'macos-lock-screen',
        message:
          'macOS reserves Control + Command + Q for locking the screen, so Talking Quill cannot receive this shortcut reliably.',
      };
    }
    if (shortcut.modifiers.meta) {
      return {
        status: 'risky',
        code: 'macos-system-or-app-command',
        message:
          'Command shortcuts often belong to macOS or the active app and may be captured before Talking Quill receives them.',
      };
    }
    if (shortcut.modifiers.alt) {
      return {
        status: 'risky',
        code: 'macos-option-text',
        message:
          'Option shortcuts can overlap symbol and accent typing. Talking Quill captures this shortcut globally while it is enabled.',
      };
    }
  }

  if (shortcutModifiersEqual(shortcut.modifiers, SHIFT_ONLY)) {
    return {
      status: 'risky',
      code: 'ordinary-shift-typing',
      message:
        'A Shift-only shortcut can capture ordinary capital-letter typing in every app while Talking Quill is enabled.',
    };
  }

  if (
    shortcut.modifiers.ctrl &&
    !shortcut.modifiers.alt &&
    !shortcut.modifiers.meta &&
    firstKey !== undefined
  ) {
    return {
      status: 'risky',
      code: 'widely-used-editing-command',
      message:
        'This is a widely used application shortcut family. Talking Quill captures it globally while it is enabled.',
    };
  }

  return { status: 'allowed' };
}
