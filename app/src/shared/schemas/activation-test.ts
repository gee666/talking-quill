import { z } from 'zod';
import { DictationProfileIdSchema } from './dictation-profiles';
import { ShortcutSchema } from './shortcut';

export const ActivationTestStateSchema = z
  .object({
    active: z.boolean(),
    phase: z.enum(['idle', 'waiting', 'pressed', 'quick', 'extended', 'observed']),
    profileId: DictationProfileIdSchema.nullable(),
    shortcut: ShortcutSchema.nullable(),
    elapsedMs: z.number().int().nonnegative(),
    unavailableReason: z
      .enum(['helper-unavailable', 'platform-unavailable', 'session-active', 'app-disabled'])
      .nullable(),
    furthestBoundary: z
      .enum([
        'hook-installed',
        'pump-alive',
        'hook-callback',
        'physical-callback',
        'registered-candidate',
        'registered-match',
        'registered-release',
        'callback-channel',
        'adapter-dequeued',
        'owner-admitted',
        'owner-flushed',
        'gateway-received',
        'v10-notification',
        'electron-received',
        'observation-accepted',
      ])
      .nullable()
      .optional(),
  })
  .strict();

export type ActivationTestState = z.infer<typeof ActivationTestStateSchema>;

export const IDLE_ACTIVATION_TEST: ActivationTestState = Object.freeze({
  active: false,
  phase: 'idle',
  profileId: null,
  shortcut: null,
  elapsedMs: 0,
  unavailableReason: null,
});
