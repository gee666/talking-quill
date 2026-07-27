import { shortcutTrigger, type Shortcut } from '../../shared/schemas/shortcut';

export function formatKeyboardShortcut(shortcut: Shortcut, platform?: string): string {
  const macOS = platform === 'darwin';
  const parts = [
    shortcut.modifiers.ctrl ? (macOS ? 'Control' : 'Ctrl') : null,
    shortcut.modifiers.alt ? (macOS ? 'Option' : 'Alt') : null,
    shortcut.modifiers.shift ? 'Shift' : null,
    shortcut.modifiers.meta ? (macOS ? 'Command' : 'Win') : null,
    ...shortcut.keys,
  ];
  return parts.filter((part): part is string => part !== null).join(' + ');
}

export function formatKeyboardShortcutWithTrigger(shortcut: Shortcut, platform?: string): string {
  return `${formatKeyboardShortcut(shortcut, platform)} (final trigger ${shortcutTrigger(shortcut)})`;
}
