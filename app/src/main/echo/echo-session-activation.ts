import { randomUUID } from 'node:crypto';
import type {
  ActivationBinding,
  HelperActivationContext,
  HelperNotification,
} from '../../shared/helper/protocol';
import { type DictationProfile } from '../../shared/schemas/dictation-profiles';
import type { HelperReadiness } from '../../shared/schemas/helper-readiness';
import { deepFreezeShortcut, shortcutsEqual } from '../../shared/schemas/shortcut';
import { type EchoSessionContext } from './echo-session-context';
import { reportOperationalFailure } from './echo-session-presentation';
import { cancel, dispatch } from './echo-session-transitions';
import { helperCaptureModeForPhase } from './session-phase';

export function acceptHelperNotification(
  context: EchoSessionContext,
  notification: HelperNotification,
): void {
  try {
    onHelperNotification(context, notification);
  } catch {
    // Treat malformed or out-of-contract native notifications as potentially capture-armed.
    // This preserves fail-open cleanup across mixed helper/app versions.
    context.captureReconciler.markNativeCaptureArmed();
    context.captureReconciler.requestBestEffort('off', context.capture.generation);
    reportOperationalFailure(context, 'Dictation could not start. Please try again.');
  }
}

function onHelperNotification(context: EchoSessionContext, notification: HelperNotification): void {
  if (
    context.disposed ||
    !context.initialized ||
    notification.method === 'paste.committed' ||
    notification.method === 'audio.input_devices_changed'
  ) {
    return;
  }
  if (notification.method === 'activation.event') {
    // HelperClient is the activation-admission authority. This check is defense in depth for
    // injected ports and prevents an unavailable owner from creating UI or starting capture.
    if (context.helper.readiness.status !== 'ready') {
      if (notification.params.phase !== 'up') {
        context.captureReconciler.requestBestEffort('off', context.capture.generation);
      }
      return;
    }
    const startsActivation = notification.params.phase !== 'up';
    if (startsActivation) {
      // Older helper versions arm session-key capture before publishing activation. Keep this
      // compatibility repair until every supported installed helper follows the new contract.
      context.captureReconciler.markNativeCaptureArmed();
    }
    if (context.profiles.shortcutCaptureActive) {
      if (startsActivation) {
        context.captureReconciler.requestBestEffort('off', context.capture.generation);
      }
      return;
    }
    if (context.activationTest.state.active) {
      context.activationTest.accept(notification, context.settings.get().dictationProfiles);
      if (startsActivation) {
        context.captureReconciler.requestBestEffort('off', context.capture.generation);
      }
      return;
    }
    if (startsActivation) {
      if (context.state.phase === 'idle') {
        const settings = context.settings.get();
        const unavailable = activationPrerequisiteError(
          settings.app.enabled,
          context.isModelReady(),
          context.helper.readiness,
          context.platform,
        );
        if (unavailable !== null) {
          context.captureReconciler.requestBestEffort('off', context.capture.generation);
          context.windows.showMain();
          reportOperationalFailure(context, unavailable);
          return;
        }
        const profile = settings.dictationProfiles.find(
          (candidate) =>
            candidate.id === notification.params.profileId &&
            shortcutsEqual(candidate.shortcut, notification.params.shortcut),
        );
        if (profile === undefined) {
          context.captureReconciler.requestBestEffort('off', context.capture.generation);
          return;
        }
        context.sessionSettings = settings;
        context.sessionProfile = deepFreezeProfile(profile);
        const now = Date.now();
        context.activeActivation =
          notification.params.phase === 'complete' ? null : freezeActivation(notification.params);
        dispatch(context, {
          type: 'shortcut-down',
          sessionId: randomUUID(),
          alternate: profile.shortcut.modifiers.shift,
          processingMode: profile.processingMode,
          activationContext: freezeActivationContext(notification.params),
          now: notification.params.phase === 'complete' ? now - notification.params.heldMs : now,
        });
        if (notification.params.phase === 'complete') {
          dispatch(context, { type: 'shortcut-up', now });
        }
      } else if (
        context.state.phase === 'recordingQuick' ||
        context.state.phase === 'recordingExtended'
      ) {
        if (
          context.sessionProfile !== null &&
          context.sessionProfile.id === notification.params.profileId
        ) {
          context.activeActivation =
            notification.params.phase === 'complete' ? null : freezeActivation(notification.params);
          dispatch(context, { type: 'submit', source: 'shortcut' });
        } else {
          context.captureReconciler.requestBestEffort(
            helperCaptureModeForPhase(context.state.phase),
            context.capture.generation,
          );
        }
      } else {
        // Active phases which do not own this shortcut explicitly restore their capture state.
        context.captureReconciler.requestBestEffort(
          helperCaptureModeForPhase(context.state.phase),
          context.capture.generation,
        );
      }
    } else {
      if (
        context.activeActivation === null ||
        !activationsEqual(context.activeActivation, notification.params)
      ) {
        return;
      }
      context.activeActivation = null;
      dispatch(context, { type: 'shortcut-up', now: Date.now() });
    }
    return;
  }
  if (notification.method !== 'session.key' || notification.params.phase !== 'down') return;
  if (notification.params.key === 'escape') cancel(context);
  else dispatch(context, { type: 'submit', source: 'enter' });
}

function deepFreezeProfile(profile: DictationProfile): Readonly<DictationProfile> {
  const clone = structuredClone(profile);
  clone.shortcut = deepFreezeShortcut(clone.shortcut);
  return Object.freeze(clone);
}

function freezeActivation(
  activation: ActivationBinding & HelperActivationContext,
): Readonly<ActivationBinding & HelperActivationContext> {
  return Object.freeze({
    profileId: activation.profileId,
    shortcut: deepFreezeShortcut(activation.shortcut),
    ...freezeActivationContext(activation),
  });
}

function freezeActivationContext(
  context: HelperActivationContext,
): Readonly<HelperActivationContext> {
  return Object.freeze({
    activationGeneration: context.activationGeneration,
    targetToken: context.targetToken,
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

function activationPrerequisiteError(
  enabled: boolean,
  modelReady: boolean,
  helperReadiness: HelperReadiness,
  platform: 'win32' | 'darwin',
): string | null {
  if (!enabled) return 'Talking Quill is turned off. Turn it on from the Dashboard.';
  if (!modelReady) {
    return 'The selected speech model is not available. Open Settings > Speech model and install or repair it.';
  }
  return helperReadinessError(helperReadiness, platform);
}

export function helperReadinessError(
  readiness: HelperReadiness,
  platform: 'win32' | 'darwin',
): string | null {
  const ownerName = platform === 'darwin' ? 'keyboard service' : 'local keyboard owner';
  if (readiness.reason === 'capture-disabled' || readiness.reason === 'owner-rollback') {
    return 'Keyboard shortcuts are safely disabled in this build.';
  }
  if (readiness.reason === 'owner-auth-failed') {
    return `The ${ownerName} could not be authenticated. Restart Talking Quill, then reinstall it if the problem continues.`;
  }
  if (readiness.reason === 'owner-security-fault') {
    return `The ${ownerName} failed a security check. Reinstall Talking Quill before using shortcuts.`;
  }
  if (readiness.reason === 'owner-draining') {
    return `Release all shortcut keys while the ${ownerName} finishes safely. Talking Quill will reconnect automatically.`;
  }
  if (readiness.reason === 'owner-maintenance') {
    return 'Keyboard shortcuts are unavailable during the update. They will return automatically when it finishes.';
  }
  if (readiness.reason === 'owner-busy') {
    return `Another Talking Quill controller is using the ${ownerName}. Close it and Talking Quill will try again automatically.`;
  }
  if (readiness.status === 'permission-required') {
    return 'Keyboard shortcuts need system permission. Open Talking Quill Settings to fix it.';
  }
  if (
    readiness.reason === 'owner-missing' ||
    readiness.reason === 'owner-degraded' ||
    readiness.reason === 'crash-loop' ||
    readiness.reason === 'unexpected-exit' ||
    readiness.reason === 'spawn-failed' ||
    readiness.reason === 'handshake-timeout' ||
    readiness.reason === 'request-timeout' ||
    readiness.reason === 'hook-fault'
  ) {
    return `The ${ownerName} is unavailable. Talking Quill will restart it automatically.`;
  }
  if (readiness.status === 'unavailable' || readiness.status === 'incompatible') {
    return `The ${ownerName} needs repair. Reinstall Talking Quill before using shortcuts.`;
  }
  return null;
}
