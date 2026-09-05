import { ECHO_TERMINAL_DISPLAY_MS } from '../../shared/constants/echo-session';
import type { ActivationTestState } from '../../shared/schemas/activation-test';
import {
  EchoSessionSnapshotSchema,
  type EchoSessionSnapshot,
} from '../../shared/schemas/echo-session';
import { type EchoSessionContext } from './echo-session-context';
import { isCapturePhase, isTerminalPhase } from './session-phase';

export function getSnapshot(context: EchoSessionContext): EchoSessionSnapshot {
  return EchoSessionSnapshotSchema.parse({
    sessionId: context.state.sessionId,
    phase:
      isCapturePhase(context.state.phase) &&
      (!context.state.captureReady || !context.state.audioReady)
        ? 'arming'
        : context.state.phase,
    dictationMode: context.state.dictationMode,
    processingMode: context.state.processingMode,
    alternate: context.state.alternate,
    rms: context.state.rms,
    elapsedMs: context.state.elapsedMs,
    transcript: context.state.transcript,
    abortReason: context.state.abortReason,
    fallbackCategory: context.state.fallbackCategory,
    completion: context.state.completion,
    message: context.state.message,
  });
}

export function publishActivationTest(
  context: EchoSessionContext,
  state: ActivationTestState,
): void {
  try {
    context.events.send('activation-test:changed', state);
  } catch {
    // Renderer publication is ancillary to activation-test ownership and cleanup.
  }
}

export function clearResetTimer(context: EchoSessionContext): void {
  if (context.resetTimer !== null) clearTimeout(context.resetTimer);
  context.resetTimer = null;
}

export function publish(context: EchoSessionContext): void {
  const snapshot = getSnapshot(context);
  try {
    context.events.send('echo:session-changed', snapshot);
  } catch {
    // A renderer disappearing cannot interrupt state-machine effects.
  }
  for (const listener of context.listeners) {
    try {
      listener(snapshot);
    } catch {
      // Subscribers are independent observers and cannot own controller progress.
    }
  }
}

export function scheduleTerminalReset(context: EchoSessionContext): void {
  if (
    context.disposed ||
    !context.captureReconciler.captureOffGuaranteed ||
    !context.teardownComplete ||
    !isTerminalPhase(context.state.phase) ||
    context.resetTimer !== null
  ) {
    return;
  }
  context.resetTimer = setTimeout(() => {
    context.resetTimer = null;
    if (
      !context.captureReconciler.captureOffGuaranteed ||
      !context.teardownComplete ||
      !isTerminalPhase(context.state.phase)
    ) {
      scheduleTerminalReset(context);
      return;
    }
    context.windows.removeWidget();
    context.dispatch({ type: 'reset' });
  }, ECHO_TERMINAL_DISPLAY_MS);
  context.resetTimer.unref();
}

export function reportOperationalFailure(context: EchoSessionContext, message: string): void {
  if (context.disposed) return;
  if (context.capture.nativeCaptureLost && context.state.phase !== 'idle') {
    // Recovery can also fail a queued profile synchronization. Preserve the
    // ongoing transcript and clipboard fallback while supervision reconnects.
    context.pendingOperationalError = message;
    return;
  }
  if (
    context.state.phase === 'inserting' ||
    context.state.phase === 'restoringClipboard' ||
    isTerminalPhase(context.state.phase)
  ) {
    // Never overwrite an insertion that may already have committed. Show this independent
    // operational failure after the current outcome has finished its truthful display.
    context.pendingOperationalError = message;
    return;
  }
  if (context.state.phase === 'idle') {
    context.dispatch({ type: 'operational-failure', message });
  } else {
    context.abort?.abort();
    context.dispatch({ type: 'fail', message });
  }
  const widgetGeneration = ++context.operationalWidgetGeneration;
  const operationalState = context.state;
  const stillCurrent = (): boolean =>
    !context.disposed &&
    widgetGeneration === context.operationalWidgetGeneration &&
    context.state === operationalState &&
    context.state.phase === 'error';
  void Promise.resolve(context.windows.createWidgetForActivation())
    .then((created) => {
      if (!stillCurrent()) return;
      if (!created || !context.windows.showWidget(context.appPreferences.widgetSize, null)) {
        context.windows.showMain();
      }
    })
    .catch(() => {
      // A late renderer failure must not reveal the main window after this error was reset.
      if (stillCurrent()) context.windows.showMain();
    });
}

export function playSound(context: EchoSessionContext): void {
  if (!context.appPreferences.soundsEnabled) return;
  try {
    context.sound();
  } catch {
    // Sound cues never affect dictation completion.
  }
}
