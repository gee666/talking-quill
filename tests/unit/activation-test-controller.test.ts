import { afterEach, describe, expect, it, vi } from 'vitest';
import { ActivationTestController } from '../../app/src/main/echo/activation-test-controller';
import type { HelperNotification } from '../../app/src/shared/helper/protocol';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { type Shortcut } from '../../app/src/shared/schemas/shortcut';

const PROMPT_SHORTCUT: Shortcut = {
  modifiers: { ctrl: true, alt: true, shift: true, meta: false },
  keys: ['Q', 'P'],
};
const WRONG_SHORTCUT: Shortcut = {
  modifiers: { ctrl: true, alt: true, shift: true, meta: false },
  keys: ['R', 'P'],
};

function activation(
  phase: 'down' | 'up',
  shortcut: Shortcut = PROMPT_SHORTCUT,
  profileId = 'prompt',
): Extract<HelperNotification, { method: 'activation.event' }> {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase, profileId, shortcut },
  };
}

function complete(
  heldMs: number,
  shortcut: Shortcut = DEFAULT_SETTINGS.dictationProfiles[0]?.shortcut ?? PROMPT_SHORTCUT,
): Extract<HelperNotification, { method: 'activation.event' }> {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase: 'complete', profileId: 'general', shortcut, heldMs },
  };
}

function profiles() {
  return DEFAULT_SETTINGS.dictationProfiles.map((profile) =>
    profile.id === 'prompt' ? { ...profile, shortcut: structuredClone(PROMPT_SHORTCUT) } : profile,
  );
}

afterEach(() => vi.useRealTimers());

describe('ActivationTestController', () => {
  it.each([
    [599, 'quick'],
    [600, 'extended'],
  ] as const)('classifies an atomic General completion at %i ms as %s', (heldMs, phase) => {
    const controller = new ActivationTestController({
      publish: vi.fn(),
      requestCaptureOff: vi.fn(),
    });
    controller.start(42, () => () => undefined, null);
    controller.accept(complete(heldMs), profiles());
    expect(controller.state).toMatchObject({
      phase,
      profileId: 'general',
      elapsedMs: heldMs,
    });
  });

  it('pairs an exact trigger-P chord and only lets the current renderer stop the test', async () => {
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

    controller.accept(activation('down'), profiles());
    await vi.advanceTimersByTimeAsync(600);
    controller.accept(activation('up'), profiles());

    expect(controller.state).toMatchObject({
      active: true,
      phase: 'extended',
      profileId: 'prompt',
      shortcut: PROMPT_SHORTCUT,
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

  it('ignores stale or wrong ups until the exact trigger-P snapshot is released', () => {
    vi.useFakeTimers();
    vi.setSystemTime(2_000);
    const controller = new ActivationTestController({
      publish: vi.fn(),
      requestCaptureOff: vi.fn(),
    });
    controller.start(42, () => () => undefined, null);

    controller.accept(activation('down'), profiles());
    vi.setSystemTime(2_100);
    controller.accept(activation('up', WRONG_SHORTCUT), profiles());
    expect(controller.state.phase).toBe('pressed');

    controller.accept(activation('up'), profiles());
    expect(controller.state).toMatchObject({
      phase: 'quick',
      shortcut: PROMPT_SHORTCUT,
      elapsedMs: 100,
    });
    controller.accept(activation('up'), profiles());
    expect(controller.state.phase).toBe('quick');
  });

  it('requires profile ownership on down but accepts the frozen binding up after reassignment', () => {
    vi.useFakeTimers();
    vi.setSystemTime(3_000);
    const controller = new ActivationTestController({
      publish: vi.fn(),
      requestCaptureOff: vi.fn(),
    });
    controller.start(42, () => () => undefined, null);

    controller.accept(activation('down', PROMPT_SHORTCUT, 'general'), profiles());
    expect(controller.state.phase).toBe('waiting');

    controller.accept(activation('down'), profiles());
    expect(controller.state.phase).toBe('pressed');
    vi.setSystemTime(3_100);
    controller.accept(activation('up', PROMPT_SHORTCUT, 'general'), []);
    expect(controller.state.phase).toBe('pressed');
    controller.accept(activation('up'), []);
    expect(controller.state).toMatchObject({
      phase: 'quick',
      profileId: 'prompt',
      shortcut: PROMPT_SHORTCUT,
      elapsedMs: 100,
    });
  });

  it('clears prior ownership and timers before publishing an unavailable replacement', () => {
    vi.useFakeTimers();
    const publish = vi.fn();
    const requestCaptureOff = vi.fn();
    const removeOwner = vi.fn();
    const ownerDestroyed: { current: (() => void) | null } = { current: null };
    const unavailableOwner = vi.fn(() => vi.fn());
    const controller = new ActivationTestController({ publish, requestCaptureOff });
    controller.start(
      42,
      (listener) => {
        ownerDestroyed.current = listener;
        return removeOwner;
      },
      null,
    );
    controller.accept(activation('down'), profiles());
    expect(vi.getTimerCount()).toBe(2);

    expect(controller.start(7, unavailableOwner, 'app-disabled')).toMatchObject({
      active: false,
      phase: 'idle',
      unavailableReason: 'app-disabled',
    });

    expect(removeOwner).toHaveBeenCalledOnce();
    expect(unavailableOwner).not.toHaveBeenCalled();
    expect(requestCaptureOff).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    ownerDestroyed.current?.();
    expect(controller.state.unavailableReason).toBe('app-disabled');
    expect(requestCaptureOff).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenLastCalledWith(controller.state);
  });

  it('safely replaces an active owner and ignores its stale destruction callback', () => {
    vi.useFakeTimers();
    const publish = vi.fn();
    const requestCaptureOff = vi.fn();
    const firstRemoveOwner = vi.fn(() => {
      throw new Error('renderer already gone');
    });
    const secondRemoveOwner = vi.fn();
    const firstDestroyed: { current: (() => void) | null } = { current: null };
    const secondDestroyed: { current: (() => void) | null } = { current: null };
    const controller = new ActivationTestController({ publish, requestCaptureOff });
    controller.start(
      42,
      (listener) => {
        firstDestroyed.current = listener;
        return firstRemoveOwner;
      },
      null,
    );
    controller.accept(activation('down'), profiles());
    expect(vi.getTimerCount()).toBe(2);

    expect(
      controller.start(
        42,
        (listener) => {
          secondDestroyed.current = listener;
          return secondRemoveOwner;
        },
        null,
      ),
    ).toMatchObject({ active: true, phase: 'waiting' });

    expect(firstRemoveOwner).toHaveBeenCalledOnce();
    expect(requestCaptureOff).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(1);
    firstDestroyed.current?.();
    expect(controller.state).toMatchObject({ active: true, phase: 'waiting' });
    secondDestroyed.current?.();
    expect(controller.state).toMatchObject({ active: false, phase: 'idle' });
    expect(secondRemoveOwner).toHaveBeenCalledOnce();
    expect(requestCaptureOff).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
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
