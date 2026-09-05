import { expect, it } from 'vitest';
import { sessionFailureCode } from '../../app/src/main/echo/session-errors';
import { CaptureClientError } from '../../app/src/main/audio/capture-window-client';
import { WhisperClientError } from '../../app/src/main/transcription/errors';

it('distinguishes capture and speech failures without exposing exception messages', () => {
  expect(sessionFailureCode(new CaptureClientError('capture-failed'))).toBe(
    'capture:capture-failed',
  );
  expect(sessionFailureCode(new WhisperClientError('WORKER_CRASHED', 'private path'))).toBe(
    'speech:WORKER_CRASHED',
  );
  expect(sessionFailureCode(new Error('private transcript'))).toBe('internal');
  expect(sessionFailureCode({ code: 'private transcript' })).toBe('internal');
});
