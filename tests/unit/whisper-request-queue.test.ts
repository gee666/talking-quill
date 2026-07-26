import { describe, expect, it, vi } from 'vitest';
import type { WhisperWorkerRequest } from '../../app/src/shared/schemas/whisper-protocol';
import { WhisperRequestQueue } from '../../app/src/workers/whisper/request-queue';

const options = { modelId: 'Xenova/whisper-small' as const, sampleRate: 16_000 as const };

describe('Whisper worker request queue', () => {
  it('bounds retained PCM and request count while reserving capacity for controls', async () => {
    let releaseFirst: (() => void) | null = null;
    const executed: string[] = [];
    const rejected: string[] = [];
    const queue = new WhisperRequestQueue({
      maximumRequests: 2,
      maximumControlRequests: 1,
      maximumPcmBytes: 8,
      execute: (request) => {
        executed.push(request.requestId);
        if (request.requestId !== 'first') return Promise.resolve();
        return new Promise<void>((resolve) => {
          releaseFirst = resolve;
        });
      },
      rejectOverload: (request) => rejected.push(request.requestId),
    });

    expect(queue.enqueue(transcribe('first', 1))).toBe(true);
    expect(queue.enqueue(transcribe('second', 1))).toBe(true);
    expect(queue.enqueue(transcribe('pcm-overload', 1))).toBe(false);
    expect(queue.enqueue(control('finish', 'session-finish'))).toBe(true);
    expect(queue.enqueue(control('control-overload', 'shutdown'))).toBe(false);
    expect(rejected).toEqual(['pcm-overload', 'control-overload']);
    queue.afterPending(() => executed.push('cleanup'));

    await vi.waitFor(() => expect(releaseFirst).not.toBeNull());
    const release = releaseFirst as (() => void) | null;
    if (release === null) throw new Error('first request did not start');
    release();
    await vi.waitFor(() => expect(executed).toEqual(['first', 'second', 'finish', 'cleanup']));

    expect(queue.enqueue(transcribe('recovered', 2))).toBe(true);
    await vi.waitFor(() => expect(executed).toContain('recovered'));
  });
});

function transcribe(requestId: string, samples: number): WhisperWorkerRequest {
  return {
    version: 1,
    requestId,
    type: 'transcribe',
    pcm: new Float32Array(samples).buffer,
    options,
  };
}

function control(
  requestId: string,
  type: 'session-cancel' | 'session-finish' | 'shutdown',
): WhisperWorkerRequest {
  return type === 'shutdown'
    ? { version: 1, requestId, type }
    : { version: 1, requestId, type, sessionId: 'session' };
}
