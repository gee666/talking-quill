import {
  ECHO_ACTIVATION_TEST_TIMEOUT_MS,
  ECHO_HOLD_THRESHOLD_MS,
} from '../../shared/constants/echo-session';
import type { ActivationBinding, HelperNotification } from '../../shared/helper/protocol';
import {
  ActivationTestStateSchema,
  IDLE_ACTIVATION_TEST,
  type ActivationTestState,
} from '../../shared/schemas/activation-test';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';
import { deepFreezeShortcut, shortcutsEqual } from '../../shared/schemas/shortcut';

type ActivationNotification = Extract<HelperNotification, { method: 'activation.event' }>;

export class ActivationTestController {
  readonly #publish: (state: ActivationTestState) => void;
  readonly #requestCaptureOff: () => void;
  #state: ActivationTestState = IDLE_ACTIVATION_TEST;
  #owner: { readonly webContentsId: number } | null = null;
  #removeOwner: (() => void) | null = null;
  #holdTimer: ReturnType<typeof setTimeout> | null = null;
  #expiryTimer: ReturnType<typeof setTimeout> | null = null;
  #pressedAt: number | null = null;
  #pressedBinding: Readonly<ActivationBinding> | null = null;

  constructor(options: {
    readonly publish: (state: ActivationTestState) => void;
    readonly requestCaptureOff: () => void;
  }) {
    this.#publish = options.publish;
    this.#requestCaptureOff = options.requestCaptureOff;
  }

  get state(): ActivationTestState {
    return this.#state;
  }

  start(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
    unavailableReason: ActivationTestState['unavailableReason'],
  ): ActivationTestState {
    this.stop();
    if (unavailableReason !== null) {
      this.#state = {
        ...IDLE_ACTIVATION_TEST,
        unavailableReason,
      };
      this.#publish(this.#state);
      return this.#state;
    }

    const owner = { webContentsId: ownerWebContentsId } as const;
    this.#owner = owner;
    let removeOwner: () => void;
    try {
      removeOwner = onDestroyed(() => {
        if (this.#owner === owner) this.stop(ownerWebContentsId);
      });
    } catch (error: unknown) {
      if (this.#owner === owner) this.#owner = null;
      throw error;
    }
    if (this.#owner !== owner) {
      removeOwner();
      return this.#state;
    }
    this.#removeOwner = removeOwner;
    this.#state = ActivationTestStateSchema.parse({
      active: true,
      phase: 'waiting',
      profileId: null,
      shortcut: null,
      elapsedMs: 0,
      unavailableReason: null,
    });
    this.#expiryTimer = setTimeout(() => this.stop(), ECHO_ACTIVATION_TEST_TIMEOUT_MS);
    this.#expiryTimer.unref();
    this.#publish(this.#state);
    return this.#state;
  }

  stop(ownerWebContentsId?: number): ActivationTestState {
    if (ownerWebContentsId !== undefined && this.#owner?.webContentsId !== ownerWebContentsId) {
      return this.#state;
    }
    this.#clearHoldTimer();
    if (this.#expiryTimer !== null) {
      clearTimeout(this.#expiryTimer);
      this.#expiryTimer = null;
    }
    this.#pressedAt = null;
    this.#pressedBinding = null;
    this.#owner = null;
    const removeOwner = this.#removeOwner;
    this.#removeOwner = null;
    try {
      removeOwner?.();
    } catch {
      // Renderer ownership cleanup cannot prevent test state and capture cleanup.
    }
    const wasActive = this.#state.active;
    this.#state = IDLE_ACTIVATION_TEST;
    if (wasActive) {
      this.#requestCaptureOff();
      this.#publish(this.#state);
    }
    return this.#state;
  }

  accept(notification: ActivationNotification, profiles: readonly DictationProfile[]): void {
    const now = Date.now();
    if (notification.params.phase === 'complete') {
      if (this.#pressedBinding !== null) return;
      const profile = profiles.find(
        (candidate) =>
          candidate.id === notification.params.profileId &&
          shortcutsEqual(candidate.shortcut, notification.params.shortcut),
      );
      if (profile === undefined) return;
      const binding = freezeActivationBinding(notification.params);
      this.#state = {
        active: true,
        phase: notification.params.heldMs >= ECHO_HOLD_THRESHOLD_MS ? 'extended' : 'quick',
        profileId: binding.profileId,
        shortcut: binding.shortcut,
        elapsedMs: notification.params.heldMs,
        unavailableReason: null,
      };
      this.#publish(this.#state);
      return;
    }
    if (notification.params.phase === 'down') {
      if (this.#pressedBinding !== null) return;
      const profile = profiles.find(
        (candidate) =>
          candidate.id === notification.params.profileId &&
          shortcutsEqual(candidate.shortcut, notification.params.shortcut),
      );
      if (profile === undefined) return;
      this.#clearHoldTimer();
      const binding = freezeActivationBinding(notification.params);
      this.#pressedAt = now;
      this.#pressedBinding = binding;
      this.#state = {
        active: true,
        phase: 'pressed',
        profileId: binding.profileId,
        shortcut: binding.shortcut,
        elapsedMs: 0,
        unavailableReason: null,
      };
      this.#holdTimer = setTimeout(() => {
        if (!this.#state.active || this.#pressedAt === null) return;
        this.#state = {
          ...this.#state,
          phase: 'extended',
          elapsedMs: Date.now() - this.#pressedAt,
        };
        this.#publish(this.#state);
      }, ECHO_HOLD_THRESHOLD_MS);
      this.#holdTimer.unref();
      this.#publish(this.#state);
      return;
    }
    if (
      this.#pressedAt === null ||
      this.#pressedBinding === null ||
      !activationBindingsEqual(this.#pressedBinding, notification.params)
    ) {
      return;
    }
    const elapsedMs = Math.max(0, now - this.#pressedAt);
    const extended = this.#state.phase === 'extended' || elapsedMs >= ECHO_HOLD_THRESHOLD_MS;
    this.#clearHoldTimer();
    this.#pressedAt = null;
    this.#pressedBinding = null;
    this.#state = {
      ...this.#state,
      phase: extended ? 'extended' : 'quick',
      elapsedMs,
    };
    this.#publish(this.#state);
  }

  #clearHoldTimer(): void {
    if (this.#holdTimer !== null) clearTimeout(this.#holdTimer);
    this.#holdTimer = null;
  }
}

function freezeActivationBinding(binding: ActivationBinding): Readonly<ActivationBinding> {
  return Object.freeze({
    profileId: binding.profileId,
    shortcut: deepFreezeShortcut(binding.shortcut),
  });
}

function activationBindingsEqual(
  left: Readonly<ActivationBinding>,
  right: ActivationBinding,
): boolean {
  return left.profileId === right.profileId && shortcutsEqual(left.shortcut, right.shortcut);
}
