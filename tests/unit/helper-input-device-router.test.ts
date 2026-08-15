import { afterEach, describe, expect, it, vi } from 'vitest';
import { installHelperInputDeviceRouter } from '../../app/src/main/helper/helper-input-device-router';

class FakeInvalidationSource {
  listener: (() => void) | null = null;
  readonly remove = vi.fn(() => {
    this.listener = null;
  });

  subscribeInputDeviceInvalidations(listener: () => void): () => void {
    this.listener = listener;
    return this.remove;
  }

  emit(): void {
    this.listener?.();
  }
}

afterEach(() => {
  vi.useRealTimers();
});

describe('helper input-device application routing', () => {
  it('coalesces a native burst into one RecordingService invalidation', async () => {
    vi.useFakeTimers();
    const source = new FakeInvalidationSource();
    const target = { invalidateInputDevices: vi.fn() };
    const dispose = installHelperInputDeviceRouter({ source, target, debounceMs: 250 });

    source.emit();
    await vi.advanceTimersByTimeAsync(200);
    source.emit();
    source.emit();
    await vi.advanceTimersByTimeAsync(249);
    expect(target.invalidateInputDevices).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);

    expect(target.invalidateInputDevices).toHaveBeenCalledOnce();
    dispose();
  });

  it('unsubscribes and cancels a pending invalidation during shutdown', async () => {
    vi.useFakeTimers();
    const source = new FakeInvalidationSource();
    const target = { invalidateInputDevices: vi.fn() };
    const dispose = installHelperInputDeviceRouter({ source, target, debounceMs: 10 });

    source.emit();
    dispose();
    dispose();
    await vi.runAllTimersAsync();

    expect(source.remove).toHaveBeenCalledOnce();
    expect(target.invalidateInputDevices).not.toHaveBeenCalled();
  });
});
