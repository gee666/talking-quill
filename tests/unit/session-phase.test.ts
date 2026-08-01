import { describe, expect, it } from 'vitest';
import { helperCaptureModeForPhase } from '../../app/src/main/echo/session-phase';
import { EchoSessionPhaseSchema } from '../../app/src/shared/schemas/echo-session';

describe('helper session capture mode', () => {
  it('captures both keys only while arming or recording and Escape through pre-commit work', () => {
    const expected = {
      idle: 'off',
      arming: 'recording',
      recordingQuick: 'recording',
      recordingExtended: 'recording',
      transcribing: 'cancel-only',
      processingSmart: 'cancel-only',
      inserting: 'cancel-only',
      restoringClipboard: 'off',
      completed: 'off',
      cancelled: 'off',
      error: 'off',
    } as const;

    for (const phase of EchoSessionPhaseSchema.options) {
      expect(helperCaptureModeForPhase(phase), phase).toBe(expected[phase]);
    }
  });
});
