import { describe, expect, it } from 'vitest';
import {
  MAX_MICROPHONE_ID_LENGTH,
  MAX_MICROPHONE_LABEL_LENGTH,
  PCM_FRAME_SAMPLES,
} from '../../app/src/shared/constants/audio';
import {
  sanitizeDeviceId,
  sanitizeMicrophoneDevices,
} from '../../app/src/renderer/capture/capture-devices';
import {
  allowsDefaultFallback,
  captureFailureCode,
  CaptureEngineError,
} from '../../app/src/renderer/capture/capture-errors';
import { CaptureEngineError as PublicCaptureEngineError } from '../../app/src/renderer/capture/capture-engine';
import { readCaptureWorkletMessage } from '../../app/src/renderer/capture/capture-worklet-message';

function device(
  deviceId: string,
  label: string,
  kind: MediaDeviceKind = 'audioinput',
): MediaDeviceInfo {
  return { deviceId, label, kind, groupId: '', toJSON: () => ({}) };
}

describe('capture device metadata', () => {
  it.each([
    undefined,
    '',
    ' \t ',
    'id\u0000',
    'id\u0085',
    'x'.repeat(MAX_MICROPHONE_ID_LENGTH + 1),
  ])('rejects an unusable id %s', (id) => expect(sanitizeDeviceId(id)).toBeNull());

  it('preserves exact valid ids, including spaces and the length boundary', () => {
    expect(sanitizeDeviceId(' device ')).toBe(' device ');
    const id = 'x'.repeat(MAX_MICROPHONE_ID_LENGTH);
    expect(sanitizeDeviceId(id)).toBe(id);
  });

  it('numbers only accepted unique audio inputs and does not mutate enumeration', () => {
    const devices = [
      device('video', '', 'videoinput'),
      device('', ''),
      device('b', '\u0000\u0085'),
      device('b', 'Duplicate'),
      device('a', ''),
      device('default', ' Default\u0085 microphone '),
      device('output', '', 'audiooutput'),
    ];
    const before = [...devices];
    expect(sanitizeMicrophoneDevices(devices)).toEqual([
      { deviceId: 'default', label: 'Default microphone', isDefault: true },
      { deviceId: 'b', label: 'Microphone 1', isDefault: false },
      { deviceId: 'a', label: 'Microphone 2', isDefault: false },
    ]);
    expect(devices).toEqual(before);
  });

  it('breaks label ties by id and trims after truncation', () => {
    const label = `${'L'.repeat(MAX_MICROPHONE_LABEL_LENGTH - 1)} extra`;
    expect(sanitizeMicrophoneDevices([device('b', label), device('a', label)])).toEqual([
      { deviceId: 'a', label: 'L'.repeat(MAX_MICROPHONE_LABEL_LENGTH - 1), isDefault: false },
      { deviceId: 'b', label: 'L'.repeat(MAX_MICROPHONE_LABEL_LENGTH - 1), isDefault: false },
    ]);
  });
});

describe('capture error policy', () => {
  it('preserves the public error constructor identity', () => {
    expect(PublicCaptureEngineError).toBe(CaptureEngineError);
    expect(new CaptureEngineError('capture-failed')).toMatchObject({
      name: 'CaptureEngineError',
      message: 'capture-failed',
      code: 'capture-failed',
    });
  });

  it.each([
    ['NotFoundError', true],
    ['NotReadableError', true],
    ['OverconstrainedError', true],
    ['NotAllowedError', false],
    ['SecurityError', false],
    ['UnknownError', false],
  ] as const)('classifies %s fallback', (name, expected) => {
    expect(allowsDefaultFallback(new DOMException('failure', name))).toBe(expected);
    expect(allowsDefaultFallback({ name })).toBe(false);
  });

  it.each([
    ['device-lost', 'device-unavailable'],
    ['system-audio-lost', 'system-audio-unavailable'],
    ['error', 'worklet-unavailable'],
    [null, 'worklet-unavailable'],
  ] as const)('maps startup stop reason %s', (reason, code) => {
    expect(captureFailureCode(reason)).toBe(code);
  });
});

describe('capture worklet message validation', () => {
  it.each([null, undefined, 1, 'frame', {}, { type: 'flush' }, { type: 'frame' }])(
    'ignores malformed or unrelated messages %s',
    (value) => expect(readCaptureWorkletMessage(value)).toBeNull(),
  );

  it.each([
    new Float32Array(),
    new Float32Array(PCM_FRAME_SAMPLES + 1),
    Float32Array.of(NaN),
    Float32Array.of(Infinity),
    Float32Array.of(-Infinity),
    Float32Array.of(1.01),
    Float32Array.of(-1.01),
    [0],
    new Float64Array(1),
  ])('rejects invalid PCM %s', (samples) => {
    expect(readCaptureWorkletMessage({ type: 'frame', samples, rms: 0 })).toBeNull();
  });

  it.each([undefined, '0', NaN, Infinity, -Infinity, -0.01, 1.01])(
    'rejects invalid RMS %s',
    (rms) => {
      expect(
        readCaptureWorkletMessage({ type: 'frame', samples: Float32Array.of(0), rms }),
      ).toBeNull();
    },
  );

  it.each([0, 1])('accepts full and partial frames without copying samples at RMS %s', (rms) => {
    for (const samples of [Float32Array.of(-1, 0, 1), new Float32Array(PCM_FRAME_SAMPLES)]) {
      const message = readCaptureWorkletMessage({ type: 'frame', samples, rms });
      expect(message).toEqual({ type: 'frame', samples, rms });
      if (message?.type !== 'frame') throw new Error('Expected frame');
      expect(message.samples).toBe(samples);
    }
  });

  it('accepts flush acknowledgements without frame fields', () => {
    expect(readCaptureWorkletMessage({ type: 'flushed' })).toEqual({ type: 'flushed' });
  });
});
