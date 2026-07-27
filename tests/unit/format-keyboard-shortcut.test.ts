import { describe, expect, it } from 'vitest';
import {
  formatKeyboardShortcut,
  formatKeyboardShortcutWithTrigger,
} from '../../app/src/renderer/main/format-keyboard-shortcut';
import type { Shortcut } from '../../app/src/shared/schemas/shortcut';

const shortcut: Shortcut = {
  modifiers: { ctrl: true, alt: true, shift: true, meta: true },
  keys: ['X', 'P'],
};

describe('keyboard shortcut formatting', () => {
  it('uses Windows modifier names and preserves held-key order', () => {
    expect(formatKeyboardShortcut(shortcut, 'win32')).toBe('Ctrl + Alt + Shift + Win + X + P');
    expect(formatKeyboardShortcut(shortcut)).toBe('Ctrl + Alt + Shift + Win + X + P');
  });

  it('uses macOS modifier names and identifies the final trigger when requested', () => {
    expect(formatKeyboardShortcut(shortcut, 'darwin')).toBe(
      'Control + Option + Shift + Command + X + P',
    );
    expect(formatKeyboardShortcutWithTrigger(shortcut, 'darwin')).toBe(
      'Control + Option + Shift + Command + X + P (final trigger P)',
    );
  });
});
