import { z } from 'zod';

/** Opaque capability minted by main for one shortcut-capture acquisition. */
export const ShortcutCaptureLeaseIdSchema = z.uuid().brand<'ShortcutCaptureLeaseId'>();

export type ShortcutCaptureLeaseId = z.infer<typeof ShortcutCaptureLeaseIdSchema>;
