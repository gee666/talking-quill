import {
  ECHO_ACTIVATION_TEST_TIMEOUT_MS,
  ECHO_HOLD_THRESHOLD_MS,
} from '../../shared/constants/echo-session';
import type {
  ActivationBinding,
  HelperActivationContext,
  HelperNotification,
  HelperRuntimeObservability,
} from '../../shared/helper/protocol';
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
  #pressedActivation: Readonly<ActivationBinding & HelperActivationContext> | null = null;
  readonly #beginPhysicalObservation: (() => Promise<HelperRuntimeObservability>) | null;
  readonly #samplePhysicalObservation: (() => Promise<HelperRuntimeObservability>) | null;
  readonly #endPhysicalObservation: (() => Promise<void>) | null;
  readonly #onObservationAccepted: (() => void) | null;
  readonly #physicalObservationUnavailable: boolean;
  #observationBaseline: HelperRuntimeObservability['registeredInput'] | null = null;
  #observationTimer: ReturnType<typeof setInterval> | null = null;
  #dedicatedObservation = false;

  constructor(options: {
    readonly publish: (state: ActivationTestState) => void;
    readonly requestCaptureOff: () => void;
    readonly beginPhysicalObservation?: () => Promise<HelperRuntimeObservability>;
    readonly samplePhysicalObservation?: () => Promise<HelperRuntimeObservability>;
    readonly endPhysicalObservation?: () => Promise<void>;
    readonly onObservationAccepted?: () => void;
    readonly physicalObservationUnavailable?: boolean;
  }) {
    this.#publish = options.publish;
    this.#requestCaptureOff = options.requestCaptureOff;
    this.#beginPhysicalObservation = options.beginPhysicalObservation ?? null;
    this.#samplePhysicalObservation = options.samplePhysicalObservation ?? null;
    this.#endPhysicalObservation = options.endPhysicalObservation ?? null;
    this.#onObservationAccepted = options.onObservationAccepted ?? null;
    this.#physicalObservationUnavailable = options.physicalObservationUnavailable ?? false;
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
    if (unavailableReason !== null || this.#physicalObservationUnavailable) {
      this.#state = {
        ...IDLE_ACTIVATION_TEST,
        unavailableReason:
          unavailableReason ??
          (this.#physicalObservationUnavailable ? 'platform-unavailable' : null),
      };
      this.#publish(this.#state);
      return this.#state;
    }

    this.#dedicatedObservation =
      this.#beginPhysicalObservation !== null && this.#samplePhysicalObservation !== null;
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
    if (this.#beginPhysicalObservation !== null && this.#samplePhysicalObservation !== null) {
      void this.#beginPhysicalObservation()
        .then((baseline) => {
          if (this.#owner !== owner) return;
          this.#observationBaseline = baseline.registeredInput;
          this.#observationTimer = setInterval(() => void this.#pollPhysicalObservation(), 100);
          this.#observationTimer.unref();
        })
        .catch(() => {
          if (this.#owner === owner) this.stop(ownerWebContentsId);
        });
    }
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
    this.#pressedActivation = null;
    this.#observationBaseline = null;
    this.#dedicatedObservation = false;
    if (this.#observationTimer !== null) {
      clearInterval(this.#observationTimer);
      this.#observationTimer = null;
    }
    if (this.#endPhysicalObservation !== null) {
      void this.#endPhysicalObservation().catch(() => undefined);
    }
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

  async #pollPhysicalObservation(): Promise<void> {
    if (!this.#state.active || this.#samplePhysicalObservation === null) return;
    const baseline = this.#observationBaseline;
    if (baseline === null) return;
    let current: HelperRuntimeObservability['registeredInput'];
    try {
      current = (await this.#samplePhysicalObservation()).registeredInput;
    } catch {
      this.stop();
      return;
    }
    if (!hasCompleteDedicatedTraversal(baseline, current)) return;
    const furthestBoundary = furthestObservationBoundary(baseline, current);
    if (furthestBoundary === null || furthestBoundary === this.#state.furthestBoundary) return;
    const firstAcceptance = this.#state.phase !== 'observed';
    this.#state = ActivationTestStateSchema.parse({
      active: true,
      phase: 'observed',
      profileId: null,
      shortcut: null,
      elapsedMs: 0,
      unavailableReason: null,
      furthestBoundary,
    });
    if (firstAcceptance) this.#onObservationAccepted?.();
    this.#publish(this.#state);
  }

  accept(notification: ActivationNotification, profiles: readonly DictationProfile[]): void {
    // Dedicated physical observation is deliberately independent of the
    // activation channel. Ignore stale/in-flight activation notifications from
    // before, during, and after disabled-mode confirmation.
    if (this.#dedicatedObservation) return;
    const now = Date.now();
    if (notification.params.phase === 'complete') {
      if (this.#pressedActivation !== null) return;
      const profile = profiles.find(
        (candidate) =>
          candidate.id === notification.params.profileId &&
          shortcutsEqual(candidate.shortcut, notification.params.shortcut),
      );
      if (profile === undefined) return;
      const activation = freezeActivation(notification.params);
      this.#state = {
        active: true,
        phase: notification.params.heldMs >= ECHO_HOLD_THRESHOLD_MS ? 'extended' : 'quick',
        profileId: activation.profileId,
        shortcut: activation.shortcut,
        elapsedMs: notification.params.heldMs,
        unavailableReason: null,
      };
      this.#publish(this.#state);
      return;
    }
    if (notification.params.phase === 'down') {
      if (this.#pressedActivation !== null) return;
      const profile = profiles.find(
        (candidate) =>
          candidate.id === notification.params.profileId &&
          shortcutsEqual(candidate.shortcut, notification.params.shortcut),
      );
      if (profile === undefined) return;
      this.#clearHoldTimer();
      const activation = freezeActivation(notification.params);
      this.#pressedAt = now;
      this.#pressedActivation = activation;
      this.#state = {
        active: true,
        phase: 'pressed',
        profileId: activation.profileId,
        shortcut: activation.shortcut,
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
      this.#pressedActivation === null ||
      !activationsEqual(this.#pressedActivation, notification.params)
    ) {
      return;
    }
    const elapsedMs = Math.max(0, now - this.#pressedAt);
    const extended = this.#state.phase === 'extended' || elapsedMs >= ECHO_HOLD_THRESHOLD_MS;
    this.#clearHoldTimer();
    this.#pressedAt = null;
    this.#pressedActivation = null;
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

export function hasCompleteDedicatedTraversal(
  baseline: HelperRuntimeObservability['registeredInput'],
  current: HelperRuntimeObservability['registeredInput'],
): boolean {
  if (
    current.callbackChannelRejected !== baseline.callbackChannelRejected ||
    current.ownerRejected !== baseline.ownerRejected
  ) {
    return false;
  }
  return [
    'physicalCallbacks',
    'registeredCandidateCallbacks',
    'registeredMatchCallbacks',
    'registeredReleaseCallbacks',
    'callbackChannelAccepted',
    'adapterDequeued',
    'ownerAdmitted',
    'ownerFlushed',
    'gatewayReceived',
    'v10NotificationAccepted',
    'electronReceived',
  ].every(
    (field) =>
      current[field as keyof HelperRuntimeObservability['registeredInput']] >
      baseline[field as keyof HelperRuntimeObservability['registeredInput']],
  );
}

export function furthestObservationBoundary(
  baseline: HelperRuntimeObservability['registeredInput'],
  current: HelperRuntimeObservability['registeredInput'],
): ActivationTestState['furthestBoundary'] {
  const boundaries = [
    ['electron-received', 'electronReceived'],
    ['v10-notification', 'v10NotificationAccepted'],
    ['gateway-received', 'gatewayReceived'],
    ['owner-flushed', 'ownerFlushed'],
    ['owner-admitted', 'ownerAdmitted'],
    ['adapter-dequeued', 'adapterDequeued'],
    ['callback-channel', 'callbackChannelAccepted'],
    ['registered-release', 'registeredReleaseCallbacks'],
    ['registered-match', 'registeredMatchCallbacks'],
    ['registered-candidate', 'registeredCandidateCallbacks'],
    ['physical-callback', 'physicalCallbacks'],
    ['hook-callback', 'hcActionCallbacks'],
    ['pump-alive', 'pumpAlive'],
    ['hook-installed', 'hookInstalled'],
  ] as const;
  return boundaries.find(([, field]) => current[field] > baseline[field])?.[0] ?? null;
}

function freezeActivation(
  activation: ActivationBinding & HelperActivationContext,
): Readonly<ActivationBinding & HelperActivationContext> {
  return Object.freeze({
    profileId: activation.profileId,
    shortcut: deepFreezeShortcut(activation.shortcut),
    activationGeneration: activation.activationGeneration,
    targetToken: activation.targetToken,
  });
}

function activationsEqual(
  left: Readonly<ActivationBinding & HelperActivationContext>,
  right: ActivationBinding & HelperActivationContext,
): boolean {
  return (
    left.activationGeneration === right.activationGeneration &&
    left.targetToken === right.targetToken &&
    left.profileId === right.profileId &&
    shortcutsEqual(left.shortcut, right.shortcut)
  );
}
