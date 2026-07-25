import { z } from 'zod';
import { ActivationKeySchema } from '../helper/protocol';
import { DictationProfileIdSchema } from './dictation-profiles';

export const ActivationTestStateSchema = z
  .object({
    active: z.boolean(),
    phase: z.enum(['idle', 'waiting', 'pressed', 'quick', 'extended']),
    profileId: DictationProfileIdSchema.nullable(),
    activationKey: ActivationKeySchema.nullable(),
    shift: z.boolean(),
    elapsedMs: z.number().int().nonnegative(),
    unavailableReason: z.enum(['helper-unavailable', 'session-active', 'app-disabled']).nullable(),
  })
  .strict();

export type ActivationTestState = z.infer<typeof ActivationTestStateSchema>;

export const IDLE_ACTIVATION_TEST: ActivationTestState = Object.freeze({
  active: false,
  phase: 'idle',
  profileId: null,
  activationKey: null,
  shift: false,
  elapsedMs: 0,
  unavailableReason: null,
});
