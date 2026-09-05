import { DEFAULT_GENERAL_PROFILE } from '../../shared/schemas/dictation-profiles';
import { type EchoSessionContext } from './echo-session-context';
import { enqueueEffect, startSmartPreparation } from './echo-session-effects';
import { abortSession, captureIsMissing } from './echo-session-operation';
import {
  publish,
  reportOperationalFailure,
  scheduleTerminalReset,
} from './echo-session-presentation';
import { helperCaptureModeForPhase, isTerminalPhase } from './session-phase';
import { reduceEchoSession, type EchoSessionEvent, type EchoSessionState } from './session-reducer';

export function stop(context: EchoSessionContext): void {
  if (context.state.phase === 'recordingQuick' || context.state.phase === 'recordingExtended') {
    dispatch(context, { type: 'submit', source: 'stop' });
  }
}

export function cancel(context: EchoSessionContext): void {
  abortSession(context, 'user-cancel');
}

export function dispatch(context: EchoSessionContext, event: EchoSessionEvent): void {
  const previous = context.state;
  const transition = reduceEchoSession(previous, event);
  if (transition.state === previous && transition.effects.length === 0) return;
  if (transition.state.phase === 'error' && previous.phase !== 'error') {
    // Failure effects are serialized behind the operation that failed. Abort that operation so
    // a non-cooperative worker promise cannot trap terminal teardown behind the effect tail.
    context.abort?.abort();
  }
  if (transition.state.phase === 'transcribing' && captureIsMissing(context)) {
    context.abort?.abort();
    context.abort = new AbortController();
  }
  context.state = transition.state;
  manageSessionTransition(context, previous, transition.state);
  context.outcomes.observeTransition(previous, transition.state);
  // Effect ownership is established before notifying observers, so a broken subscriber cannot
  // strand the state machine after its state has already advanced.
  for (const effect of transition.effects) enqueueEffect(context, effect);
  publish(context);
  if (
    transition.state.phase === 'error' &&
    transition.state.sessionId === null &&
    previous.phase === 'idle'
  ) {
    scheduleTerminalReset(context);
  }
  if (event.type === 'reset' && context.pendingOperationalError !== null) {
    const message = context.pendingOperationalError;
    context.pendingOperationalError = null;
    reportOperationalFailure(context, message);
  }
}

function manageSessionTransition(
  context: EchoSessionContext,
  previous: EchoSessionState,
  next: EchoSessionState,
): void {
  if (next.phase === 'idle' || isTerminalPhase(next.phase)) {
    context.activeActivation = null;
    context.smartPreparation = null;
    if (next.phase !== 'completed') context.outcomes.discardSmartSession();
  }
  if (previous.phase === 'idle' && next.phase === 'arming') {
    context.capture.beginGeneration();
    context.abort = new AbortController();
    context.teardownComplete = false;
    const sessionSettings = context.sessionSettings ?? context.settings.get();
    context.sessionSettings = sessionSettings;
    const processingMode = context.state.processingMode;
    if (processingMode === null) throw new Error('Session processing mode was unavailable');
    context.outcomes.beginSession(
      sessionSettings,
      context.sessionProfile ?? DEFAULT_GENERAL_PROFILE,
      processingMode,
    );
    startSmartPreparation(context, next);
    context.capture.arm(sessionSettings);
  }
  const previousHelperMode = helperCaptureModeForPhase(previous.phase);
  const nextHelperMode = helperCaptureModeForPhase(next.phase);
  if (
    previousHelperMode !== nextHelperMode &&
    !(previous.phase === 'idle' && next.phase === 'arming')
  ) {
    context.captureReconciler.requestBestEffort(nextHelperMode, context.capture.generation);
  }
  context.capture.observeTransition(previous, next);
}
