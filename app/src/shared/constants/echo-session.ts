export const ECHO_HOLD_THRESHOLD_MS = 600 as const;
export const ECHO_CLIPBOARD_RESTORE_MS = 300 as const;
export const ECHO_TERMINAL_DISPLAY_MS = 1_200 as const;
export const ECHO_LEVEL_EVENT_INTERVAL_MS = 50 as const;
export const ECHO_ACTIVATION_TEST_TIMEOUT_MS = 10_000 as const;

export const WIDGET_DIMENSIONS = Object.freeze({
  default: Object.freeze({ width: 360, height: 96 }),
  large: Object.freeze({ width: 440, height: 112 }),
  huge: Object.freeze({ width: 520, height: 136 }),
  max: Object.freeze({ width: 640, height: 160 }),
});
