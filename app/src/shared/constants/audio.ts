export const CAPTURE_PORT_PROTOCOL_VERSION = 1 as const;
export const CAPTURE_PORT_WINDOW_MESSAGE = 'talking-quill:capture-port' as const;
export const PCM_SAMPLE_RATE = 16_000 as const;
export const PCM_CHANNEL_COUNT = 1 as const;
export const PCM_FRAME_SAMPLES = 320 as const;
export const PCM_FRAME_DURATION_MS = 20 as const;
export const CAPTURE_WORKLET_PROCESSOR_NAME = 'talking-quill-capture' as const;
export const CAPTURE_WORKLET_FLUSH_TIMEOUT_MS = 500 as const;
export const MAX_MICROPHONE_DEVICES = 128 as const;
export const MAX_MICROPHONE_ID_LENGTH = 1_024 as const;
export const MAX_MICROPHONE_LABEL_LENGTH = 256 as const;

export const SPEECH_ARMING_MS = 300 as const;

export const SILENCE_PRESET_MS = Object.freeze({
  aggressive: 1_000,
  average: 1_800,
  relaxed: 3_000,
});

export const SESSION_CAP_MS = Object.freeze({
  quick: 2 * 60 * 1_000,
  extended: 30 * 60 * 1_000,
});

export const CAPTURE_COMMAND_TIMEOUT_MS = 10_000 as const;
export const CAPTURE_CANCEL_TIMEOUT_MS = 1_000 as const;
export const DEVICE_CHANGE_DEBOUNCE_MS = 250 as const;
export const MICROPHONE_AUTHORIZATION_TTL_MS = 15_000 as const;
