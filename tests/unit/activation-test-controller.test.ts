import { afterEach, describe, expect, it, vi } from 'vitest';
import { ActivationTestController } from '../../app/src/main/echo/activation-test-controller';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import type { HelperNotification } from '../../app/src/shared/helper/protocol';

function activation(
  phase: 'down' | 'up',
  shift = false,
): Extract<HelperNotification, { method: 'activation.event' }> {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase, key: 'Z', shift },
  };
}

afterEach(() => vi.useRealTimers());

describe('ActivationTestController', () => {
  it('owns gesture timing and only lets the current renderer stop the test', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const publish = vi.fn();
    const requestCaptureOff = vi.fn();
    const removeOwner = vi.fn();
    const ownerDestroyed: { current: (() => void) | null } = { current: null };
    const controller = new ActivationTestController({ publish, requestCaptureOff });

    expect(
      controller.start(
        42,
        (listener) => {
          ownerDestroyed.current = listener;
          return removeOwner;
        },
        null,
      ),
    ).toMatchObject({ active: true, phase: 'waiting' });

    controller.accept(activation('down', true), DEFAULT_SETTINGS.dictationProfiles);
    await vi.advanceTimersByTimeAsync(600);
    controller.accept(activation('up', true), DEFAULT_SETTINGS.dictationProfiles);

    expect(controller.state).toMatchObject({
      active: true,
      phase: 'extended',
      profileId: 'prompt',
      activationKey: 'Z',
      shift: true,
      elapsedMs: 600,
    });
    expect(controller.stop(7)).toBe(controller.state);
    expect(requestCaptureOff).not.toHaveBeenCalled();

    ownerDestroyed.current?.();
    expect(controller.state).toMatchObject({ active: false, phase: 'idle' });
    expect(requestCaptureOff).toHaveBeenCalledOnce();
    expect(removeOwner).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenLastCalledWith(controller.state);
  });

  it('publishes refusal state without creating test ownership', () => {
    const publish = vi.fn();
    const requestCaptureOff = vi.fn();
    const onDestroyed = vi.fn(() => vi.fn());
    const controller = new ActivationTestController({ publish, requestCaptureOff });

    expect(controller.start(42, onDestroyed, 'helper-unavailable')).toMatchObject({
      active: false,
      phase: 'idle',
      unavailableReason: 'helper-unavailable',
    });
    expect(onDestroyed).not.toHaveBeenCalled();
    expect(publish).toHaveBeenCalledOnce();
    expect(requestCaptureOff).not.toHaveBeenCalled();
  });
});
