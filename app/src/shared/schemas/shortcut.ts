import { z } from 'zod';

export const ShortcutKeySchema = z.enum([
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'O',
  'P',
  'Q',
  'R',
  'S',
  'T',
  'U',
  'V',
  'W',
  'X',
  'Y',
  'Z',
]);

export const ShortcutModifiersSchema = z
  .object({
    ctrl: z.boolean(),
    alt: z.boolean(),
    shift: z.boolean(),
    meta: z.boolean(),
  })
  .strict();

export const ShortcutSchema = z
  .object({
    modifiers: ShortcutModifiersSchema,
    keys: z
      .array(ShortcutKeySchema)
      .min(1)
      .max(ShortcutKeySchema.options.length)
      .superRefine((keys, context) => {
        const seen = new Set<ShortcutKey>();
        for (const [index, key] of keys.entries()) {
          if (seen.has(key)) {
            context.addIssue({
              code: 'custom',
              path: [index],
              message: 'Shortcut keys must be unique',
            });
          }
          seen.add(key);
        }
      }),
  })
  .strict()
  .superRefine((shortcut, context) => {
    const { ctrl, alt, shift, meta } = shortcut.modifiers;
    if (!ctrl && !alt && !shift && !meta) {
      context.addIssue({
        code: 'custom',
        path: ['modifiers'],
        message: 'A shortcut must contain at least one modifier',
      });
    }
  });

export type ShortcutKey = z.infer<typeof ShortcutKeySchema>;
export type ShortcutModifiers = z.infer<typeof ShortcutModifiersSchema>;
export type Shortcut = z.infer<typeof ShortcutSchema>;

export function shortcutFromLegacyActivation(key: ShortcutKey, shift: boolean): Shortcut {
  return {
    modifiers: { ctrl: false, alt: true, shift, meta: false },
    keys: [key],
  };
}

export function deepFreezeShortcut(shortcut: Shortcut): Shortcut {
  const clone = structuredClone(shortcut);
  Object.freeze(clone.modifiers);
  Object.freeze(clone.keys);
  return Object.freeze(clone);
}

export function shortcutTrigger(shortcut: Shortcut): ShortcutKey {
  const trigger = shortcut.keys.at(-1);
  if (trigger === undefined) throw new Error('A shortcut must contain a trigger key');
  return trigger;
}

export function shortcutIdentity(shortcut: Shortcut): string {
  const { ctrl, alt, shift, meta } = shortcut.modifiers;
  return `${ctrl ? '1' : '0'}${alt ? '1' : '0'}${shift ? '1' : '0'}${meta ? '1' : '0'}:${shortcut.keys.join('')}`;
}

export function shortcutsEqual(left: Shortcut, right: Shortcut): boolean {
  return shortcutIdentity(left) === shortcutIdentity(right);
}

export function shortcutModifiersEqual(left: ShortcutModifiers, right: ShortcutModifiers): boolean {
  return (
    left.ctrl === right.ctrl &&
    left.alt === right.alt &&
    left.shift === right.shift &&
    left.meta === right.meta
  );
}

export function shortcutsConflict(left: Shortcut, right: Shortcut): boolean {
  if (!shortcutModifiersEqual(left.modifiers, right.modifiers)) return false;
  const prefixLength = Math.min(left.keys.length, right.keys.length);
  for (let index = 0; index < prefixLength; index += 1) {
    if (left.keys[index] !== right.keys[index]) return false;
  }
  return true;
}
