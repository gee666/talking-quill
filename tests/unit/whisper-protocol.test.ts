import { describe, expect, it } from 'vitest';
import {
  WhisperWorkerRequestSchema,
  WhisperWorkerResponseSchema,
} from '../../app/src/shared/schemas/whisper-protocol';
import {
  TRANSCRIPT_MAX_CHARACTERS,
  TRANSCRIPT_MAX_UTF8_BYTES,
} from '../../app/src/shared/schemas/transcription';

const options = {
  modelId: 'Xenova/whisper-small',
  sampleRate: 16_000,
  language: 'ru',
};

describe('Whisper worker protocol', () => {
  it('requires v2, bounded 16 kHz Float32 PCM, and auto or a supported source language', () => {
    const valid = {
      version: 2,
      requestId: 'request-1',
      type: 'transcribe',
      pcm: new Float32Array([0, 0.1]).buffer,
      options,
    };
    expect(WhisperWorkerRequestSchema.safeParse(valid).success).toBe(true);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...valid,
        options: { ...valid.options, language: 'auto' },
      }).success,
    ).toBe(true);
    expect(WhisperWorkerRequestSchema.safeParse({ ...valid, version: 1 }).success).toBe(false);
    expect(WhisperWorkerRequestSchema.safeParse({ ...valid, extra: true }).success).toBe(false);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...valid,
        options: { ...valid.options, sampleRate: 44_100 },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...valid,
        options: { modelId: valid.options.modelId, sampleRate: 16_000 },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...valid,
        options: { ...valid.options, language: 'x' },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...valid,
        options: { ...valid.options, task: 'translate' },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerRequestSchema.safeParse({ ...valid, pcm: new ArrayBuffer(3) }).success,
    ).toBe(false);
  });

  it('rejects the removed parent-supplied model authorization path', () => {
    expect(
      WhisperWorkerRequestSchema.safeParse({
        version: 2,
        requestId: 'forged-authorization',
        type: 'model-authorize',
        authorization: {
          capability: '00000000-0000-4000-8000-000000000000',
          modelId: 'Xenova/whisper-small',
          revision: 'a'.repeat(40),
          files: [],
        },
      }).success,
    ).toBe(false);
  });

  it('requires typed acknowledgement operations and guarded readiness', () => {
    const envelope = { version: 2, requestId: 'response-1', ok: true };
    expect(
      WhisperWorkerResponseSchema.safeParse({
        ...envelope,
        result: { type: 'acknowledged', operation: 'session-push' },
      }).success,
    ).toBe(true);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        version: 2,
        requestId: 'cancel-request',
        type: 'session-cancel',
        sessionId: 'session-1',
      }).success,
    ).toBe(true);
    expect(
      WhisperWorkerResponseSchema.safeParse({
        ...envelope,
        result: { type: 'acknowledged', operation: 'session-cancel' },
      }).success,
    ).toBe(true);
    expect(
      WhisperWorkerResponseSchema.safeParse({
        ...envelope,
        result: { type: 'acknowledged' },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerResponseSchema.safeParse({
        ...envelope,
        result: { type: 'ready' },
      }).success,
    ).toBe(false);
    expect(
      WhisperWorkerResponseSchema.safeParse({
        ...envelope,
        result: { type: 'ready', networkGuarded: true, networkProbeCompleted: false },
      }).success,
    ).toBe(true);
  });

  it('bounds worker transcript responses by characters and UTF-8 bytes', () => {
    const response = (text: string) => ({
      version: 2,
      requestId: 'transcript-response',
      ok: true,
      result: {
        type: 'transcription',
        value: {
          text,
          modelId: 'Xenova/whisper-small',
          durationMs: 1,
          pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
        },
      },
    });
    expect(
      WhisperWorkerResponseSchema.safeParse(response('a'.repeat(TRANSCRIPT_MAX_UTF8_BYTES)))
        .success,
    ).toBe(true);
    expect(
      WhisperWorkerResponseSchema.safeParse(response('é'.repeat(TRANSCRIPT_MAX_UTF8_BYTES / 2 + 1)))
        .success,
    ).toBe(false);
    expect(
      WhisperWorkerResponseSchema.safeParse(response('a'.repeat(TRANSCRIPT_MAX_CHARACTERS + 1)))
        .success,
    ).toBe(false);
  });

  it('bounds each streaming push independently of the cumulative session cap', () => {
    const base = {
      version: 2,
      requestId: 'push-1',
      type: 'session-push',
      sessionId: 'session-1',
    };
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...base,
        pcm: new Float32Array(10 * 16_000).buffer,
      }).success,
    ).toBe(true);
    expect(
      WhisperWorkerRequestSchema.safeParse({
        ...base,
        pcm: new Float32Array(10 * 16_000 + 1).buffer,
      }).success,
    ).toBe(false);
  });
});
