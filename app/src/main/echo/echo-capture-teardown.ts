import { CAPTURE_CANCEL_TIMEOUT_MS } from '../../shared/constants/audio';
import type { HelperSessionCaptureMode } from '../../shared/helper/protocol';
import { ownsSession, type EchoCaptureContext } from './echo-capture-context';
import { clearRecordingTimers } from './echo-capture-startup';
import { normalizeOperationError, operationError, withSoftTimeout } from './echo-operation';
import { helperCaptureModeForPhase } from './session-phase';

const MODEL_USE_SETTLE_TIMEOUT_MS = 1_000;
const STREAM_CANCEL_TIMEOUT_MS = 1_000;

export async function stopCapture(
  context: EchoCaptureContext,
  owner = context.sessionOwner,
  helperMode: HelperSessionCaptureMode = helperCaptureModeForPhase(context.getState().phase),
): Promise<void> {
  const sessionOwned = owner !== null && ownsSession(context, owner);
  const generation = owner?.generation ?? context.generation;
  const captureId = sessionOwned ? context.captureId : null;
  const shouldCancelOpening = sessionOwned && captureId === null && owner.captureOpening;
  const shouldStopRecording =
    !context.captureStopCompleted && (captureId !== null || shouldCancelOpening);
  if (sessionOwned) context.captureStopping = captureId !== null && shouldStopRecording;
  const recordingStop = shouldStopRecording
    ? withSoftTimeout(
        operationError(
          () => context.recording.stopDictation(captureId ?? undefined),
          'Microphone stop failed',
        ),
        CAPTURE_CANCEL_TIMEOUT_MS,
        new Error('Microphone stop timed out'),
      )
    : Promise.resolve(null);
  // Change native key capture immediately rather than waiting for microphone teardown. Both
  // boundaries are independently bounded because custom ports need not honor production limits.
  const helperStop = withSoftTimeout(
    operationError(
      () => context.captureReconciler.request(helperMode, generation),
      'Helper capture mode change failed',
    ),
    CAPTURE_CANCEL_TIMEOUT_MS,
    new Error('Helper capture mode change timed out'),
  );
  let errors: readonly Error[];
  try {
    const [recordingError, helperError] = await Promise.all([recordingStop, helperStop]);
    if (sessionOwned && ownsSession(context, owner) && shouldStopRecording) {
      context.captureStopCompleted = recordingError === null;
    }
    // The supervisor owns a disconnected helper. Its failed acknowledgement must not
    // discard audio that the independent capture renderer successfully drained.
    errors = [recordingError, context.nativeCaptureLost ? null : helperError].filter(
      (error): error is Error => error !== null,
    );
  } finally {
    if (sessionOwned && ownsSession(context, owner)) context.captureStopping = false;
  }
  const firstError = errors[0];
  if (errors.length === 1 && firstError !== undefined) throw firstError;
  if (errors.length > 1) throw new AggregateError(errors, 'Capture stop failed');
}

export async function performTeardown(
  context: EchoCaptureContext,
  afterTimersCleared: () => void,
): Promise<void> {
  const owner = context.sessionOwner;
  const opening = context.streamOpening;
  const ownedStream = context.stream;
  const streamTail = context.streamTail;
  const modelUseOpening = context.modelUseOpening;
  const warmupOpening = context.warmupOpening;
  const modelUse = context.modelUse;
  if (owner !== null) owner.active = false;
  clearRecordingTimers(context);
  afterTimersCleared();
  let captureError: unknown = null;
  try {
    await stopCapture(context, owner);
  } catch (error: unknown) {
    captureError = error;
  } finally {
    const stream =
      ownedStream ??
      (opening === null
        ? null
        : await withSoftTimeout(
            opening.catch(() => null),
            STREAM_CANCEL_TIMEOUT_MS,
            null,
          ));
    const streamCancellation =
      stream !== null && context.getState().phase !== 'completed'
        ? stream.cancel().catch(() => undefined)
        : Promise.resolve();
    await Promise.all([
      withSoftTimeout(
        streamTail.catch(() => undefined),
        STREAM_CANCEL_TIMEOUT_MS,
        undefined,
      ),
      withSoftTimeout(streamCancellation, STREAM_CANCEL_TIMEOUT_MS, undefined),
    ]);
    await Promise.all([
      withSoftTimeout(
        modelUseOpening.catch(() => undefined),
        MODEL_USE_SETTLE_TIMEOUT_MS,
        undefined,
      ),
      withSoftTimeout(
        warmupOpening.catch(() => undefined),
        MODEL_USE_SETTLE_TIMEOUT_MS,
        undefined,
      ),
    ]);
    modelUse?.release();
    if (ownsSession(context, owner)) {
      context.sessionOwner = null;
      context.captureId = null;
      context.captureStopCompleted = false;
      context.captureStopping = false;
      context.stream = null;
      context.streamOpening = null;
      context.modelUse = null;
      context.modelUseOpening = Promise.resolve();
      context.warmupOpening = Promise.resolve();
      context.pcmChunks = [];
      context.totalSamples = 0;
      context.streamedSamples = 0;
      context.discardedSamples = 0;
      context.streamPushPending = false;
      context.streamFailure = null;
      context.silence = null;
    }
  }
  if (captureError !== null) {
    throw normalizeOperationError(captureError, 'Capture teardown failed');
  }
}
