export interface HelperInputDeviceInvalidationSource {
  subscribeInputDeviceInvalidations(listener: () => void): () => void;
}

export interface InputDeviceInvalidationTarget {
  invalidateInputDevices(): void;
}

export interface HelperInputDeviceRouterOptions {
  readonly source: HelperInputDeviceInvalidationSource;
  readonly target: InputDeviceInvalidationTarget;
  readonly debounceMs?: number;
}

const DEFAULT_DEBOUNCE_MS = 250;

/** Coalesces native endpoint bursts and routes them into the audio lifecycle. */
export function installHelperInputDeviceRouter(
  options: HelperInputDeviceRouterOptions,
): () => void {
  let disposed = false;
  let timer: NodeJS.Timeout | null = null;
  const removeListener = options.source.subscribeInputDeviceInvalidations(() => {
    if (disposed) return;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      if (!disposed) options.target.invalidateInputDevices();
    }, options.debounceMs ?? DEFAULT_DEBOUNCE_MS);
    timer.unref();
  });

  return () => {
    if (disposed) return;
    disposed = true;
    removeListener();
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
}
