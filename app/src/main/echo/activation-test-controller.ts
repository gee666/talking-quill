import {
  ECHO_ACTIVATION_TEST_TIMEOUT_MS,
  ECHO_HOLD_THRESHOLD_MS,
} from '../../shared/constants/echo-session';
import type { HelperNotification } from '../../shared/helper/protocol';
import {
  ActivationTestStateSchema,
  IDLE_ACTIVATION_TEST,
  type ActivationTestState,
} from '../../shared/schemas/activation-test';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';

type ActivationNotification = Extract<HelperNotification, { method: 'activation.event' }>;

export class ActivationTestController {
  readonly #publish: (state: ActivationTestState) => void;
  readonly #requestCaptureOff: () => void;
  #state: ActivationTestState = IDLE_ACTIVATION_TEST;
  #owner: number | null = null;
  #removeOwner: (() => void) | null = null;
  #holdTimer: ReturnType<typeof setTimeout> | null = null;
  #expiryTimer: ReturnType<typeof setTimeout> | null = null;
  #pressedAt: number | null = null;

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
    if (unavailableReason !== null) {
      this.#state = {
        ...IDLE_ACTIVATION_TEST,
        unavailableReason,
      };
      this.#publish(this.#state);
      return this.#state;
    }
    this.stop();
    this.#owner = ownerWebContentsId;
    this.#removeOwner = onDestroyed(() => this.stop(ownerWebContentsId));
    this.#state = ActivationTestStateSchema.parse({
      active: true,
      phase: 'waiting',
      profileId: null,
      activationKey: null,
      shift: false,
      elapsedMs: 0,
      unavailableReason: null,
    });
    this.#expiryTimer = setTimeout(() => this.stop(), ECHO_ACTIVATION_TEST_TIMEOUT_MS);
    this.#expiryTimer.unref();
    this.#publish(this.#state);
    return this.#state;
  }

  stop(ownerWebContentsId?: number): ActivationTestState {
    if (ownerWebContentsId !== undefined && this.#owner !== ownerWebContentsId) {
      return this.#state;
    }
    this.#clearHoldTimer();
    if (this.#expiryTimer !== null) {
      clearTimeout(this.#expiryTimer);
      this.#expiryTimer = null;
    }
    this.#pressedAt = null;
    this.#owner = null;
    this.#removeOwner?.();
    this.#removeOwner = null;
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
    if (notification.params.phase === 'down') {
      const profile = profiles.find(
        (candidate) =>
          candidate.activationKey === notification.params.key &&
          candidate.shift === notification.params.shift,
      );
      if (profile === undefined) return;
      this.#clearHoldTimer();
      this.#pressedAt = now;
      this.#state = {
        active: true,
        phase: 'pressed',
        profileId: profile.id,
        activationKey: profile.activationKey,
        shift: profile.shift,
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
    if (this.#pressedAt === null) return;
    const elapsedMs = Math.max(0, now - this.#pressedAt);
    const extended = this.#state.phase === 'extended' || elapsedMs >= ECHO_HOLD_THRESHOLD_MS;
    this.#clearHoldTimer();
    this.#pressedAt = null;
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
