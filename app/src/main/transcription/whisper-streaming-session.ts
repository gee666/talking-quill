import { randomUUID } from 'node:crypto';
import {
  WHISPER_MAX_PUSH_SAMPLES,
  WHISPER_MAX_SAMPLES,
  WHISPER_PROTOCOL_VERSION,
} from '../../shared/constants/whisper';
import type { TranscriptionOptions, TranscriptionResult } from '../../shared/schemas/transcription';
import { WhisperClientError } from './errors';
import type { WhisperWorkerSupervisor } from './whisper-worker-supervisor';
import {
  CONTROL_REQUEST_TIMEOUT_MS,
  acceptsAcknowledgement,
  assertAcknowledged,
  waitForDispatchTurn,
} from './whisper-worker-requests-support';
import {
  inferenceTimeoutMs,
  combineAbortSignals,
  streamingPushPlan,
  assertPcmLength,
  copyPcm,
  once,
} from './whisper-streaming-audio';

export { inferenceTimeoutMs } from './whisper-streaming-audio';

const MAX_PENDING_PUSHES = 8;

export interface WhisperStreamingSession {
  readonly id: string;
  push(pcm: Float32Array, signal?: AbortSignal): Promise<void>;
  finish(signal?: AbortSignal): Promise<TranscriptionResult>;
  cancel(): Promise<void>;
}

interface ModelUseLease {
  release(): void;
}

export async function openWhisperStreamingSession(options: {
  readonly supervisor: WhisperWorkerSupervisor;
  readonly transcriptionOptions: TranscriptionOptions;
  readonly expectedGeneration: number | undefined;
  readonly use: ModelUseLease;
  readonly signal?: AbortSignal | undefined;
}): Promise<WhisperStreamingSession> {
  const { supervisor, transcriptionOptions, expectedGeneration, use, signal } = options;
  let sessionGeneration = 0;
  try {
    const sessionId = randomUUID();
    const opened = await supervisor.request(
      (requestId) => ({
        version: WHISPER_PROTOCOL_VERSION,
        requestId,
        type: 'session-open',
        sessionId,
        options: transcriptionOptions,
      }),
      {
        signal,
        timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
        expectedGeneration,
        accepts: acceptsAcknowledgement('session-open'),
        captureGeneration: (generation) => {
          sessionGeneration = generation;
        },
      },
    );
    assertAcknowledged(opened, 'session-open');
    if (!supervisor.isOperationalGeneration(sessionGeneration)) {
      throw new WhisperClientError('WORKER_CRASHED', 'Streaming worker exited during open.');
    }
    let closed = false;
    let activePushes = 0;
    let totalSamples = 0;
    let bufferedSamples = 0;
    let pushTail = Promise.resolve();
    let pushFailed = false;
    let firstPushError: unknown;
    const failureCancellation: { value: Promise<void> | null } = { value: null };
    const getFailureCancellation = (): Promise<void> | null => failureCancellation.value;
    const finishState: { settlement: Promise<void> | null } = { settlement: null };
    let cancellationPromise: Promise<void> | null = null;
    let workerCancellationPromise: Promise<void> | null = null;
    const pendingSessionRequestIds = new Set<string>();
    const sessionController = new AbortController();
    const releaseUse = once(() => use.release());
    const releaseSessionUse = () => supervisor.releaseUseWhenSafe(sessionGeneration, releaseUse);
    supervisor.registerSessionLease(sessionGeneration, releaseUse);
    const closeSession = () => {
      if (closed) return false;
      closed = true;
      supervisor.unregisterSessionLease(sessionGeneration, releaseUse);
      return true;
    };
    const cancelWorkerSession = async () => {
      if (!supervisor.isCurrentGeneration(sessionGeneration)) return;
      try {
        const cancelled = await supervisor.request(
          (requestId) => ({
            version: WHISPER_PROTOCOL_VERSION,
            requestId,
            type: 'session-cancel',
            sessionId,
          }),
          {
            timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
            expectedGeneration: sessionGeneration,
            accepts: acceptsAcknowledgement('session-cancel'),
          },
        );
        assertAcknowledged(cancelled, 'session-cancel');
      } catch (error: unknown) {
        await supervisor.beginTermination(
          sessionGeneration,
          'cancel',
          error instanceof WhisperClientError && error.code === 'CANCELLED'
            ? error
            : new WhisperClientError('CANCELLED', 'Streaming transcription was cancelled.'),
          false,
          new Set(),
        );
      }
    };
    const ensureWorkerSessionCancelled = (): Promise<void> => {
      workerCancellationPromise ??= cancelWorkerSession();
      return workerCancellationPromise;
    };
    const latchPushFailure = (error: unknown) => {
      if (pushFailed) return;
      pushFailed = true;
      firstPushError = error;
      closeSession();
      failureCancellation.value = ensureWorkerSessionCancelled().finally(releaseSessionUse);
    };
    const throwIfPushFailed = () => {
      if (pushFailed) throw firstPushError;
    };
    return {
      id: sessionId,
      push: async (pcm, pushSignal) => {
        throwIfPushFailed();
        if (closed) throw new WhisperClientError('CANCELLED', 'Streaming session is closed.');
        if (!supervisor.isCurrentGeneration(sessionGeneration)) {
          closeSession();
          releaseSessionUse();
          throw new WhisperClientError('WORKER_CRASHED', 'Streaming worker generation changed.');
        }
        assertPcmLength(pcm, WHISPER_MAX_PUSH_SAMPLES, 'PCM push is invalid or too large.');
        if (totalSamples + pcm.length > WHISPER_MAX_SAMPLES) {
          throw new WhisperClientError('INVALID_AUDIO', 'PCM exceeds maximum session duration.');
        }
        if (activePushes >= MAX_PENDING_PUSHES) {
          throw new WhisperClientError('INVALID_AUDIO', 'Too many PCM pushes are pending.');
        }
        totalSamples += pcm.length;
        activePushes += 1;
        const buffer = copyPcm(pcm);
        const pushOperationSignal = combineAbortSignals(pushSignal, sessionController.signal);
        const pushing = waitForDispatchTurn(pushTail, pushOperationSignal).then(async () => {
          throwIfPushFailed();
          const pushRequestId: { value: string | null } = { value: null };
          const pushPlan = streamingPushPlan(bufferedSamples, pcm.length);
          try {
            const pushed = await supervisor.request(
              (requestId) => ({
                version: WHISPER_PROTOCOL_VERSION,
                requestId,
                type: 'session-push',
                sessionId,
                pcm: buffer,
              }),
              {
                signal: pushOperationSignal,
                timeoutMs: pushPlan.timeoutMs,
                expectedGeneration: sessionGeneration,
                accepts: acceptsAcknowledgement('session-push'),
                captureRequestId: (requestId) => {
                  pushRequestId.value = requestId;
                },
                onDispatched: (requestId) => pendingSessionRequestIds.add(requestId),
              },
            );
            assertAcknowledged(pushed, 'session-push');
            bufferedSamples = pushPlan.remainingSamples;
          } catch (error: unknown) {
            latchPushFailure(error);
            throw firstPushError;
          } finally {
            if (pushRequestId.value !== null) {
              pendingSessionRequestIds.delete(pushRequestId.value);
            }
          }
        });
        pushTail = pushing.catch(() => undefined);
        try {
          await pushing;
        } catch (error: unknown) {
          totalSamples -= pcm.length;
          throw error;
        } finally {
          activePushes -= 1;
        }
      },
      finish: async (finishSignal) => {
        throwIfPushFailed();
        if (!closeSession()) {
          throw new WhisperClientError('CANCELLED', 'Streaming session is closed.');
        }
        let releaseDeferred = false;
        let resolveFinishSettlement!: () => void;
        const finishSettlement = new Promise<void>((resolve) => {
          resolveFinishSettlement = resolve;
        });
        finishState.settlement = finishSettlement;
        const abortSession = (): void => sessionController.abort('streaming finish cancelled');
        if (finishSignal?.aborted === true) abortSession();
        else finishSignal?.addEventListener('abort', abortSession, { once: true });
        const finishOperationSignal = combineAbortSignals(finishSignal, sessionController.signal);
        try {
          await waitForDispatchTurn(pushTail, finishOperationSignal);
          throwIfPushFailed();
          if (!supervisor.isCurrentGeneration(sessionGeneration)) {
            throw new WhisperClientError('WORKER_CRASHED', 'Streaming worker generation changed.');
          }
          const result = await supervisor.request(
            (requestId) => ({
              version: WHISPER_PROTOCOL_VERSION,
              requestId,
              type: 'session-finish',
              sessionId,
            }),
            {
              signal: finishOperationSignal,
              timeoutMs:
                bufferedSamples === 0
                  ? CONTROL_REQUEST_TIMEOUT_MS
                  : inferenceTimeoutMs(bufferedSamples),
              expectedGeneration: sessionGeneration,
              accepts: (result) => result.type === 'transcription',
            },
          );
          if (result.type !== 'transcription') {
            throw new WhisperClientError('PROTOCOL_ERROR', 'Worker returned the wrong response.');
          }
          return result.value;
        } catch (error: unknown) {
          if (finishOperationSignal.aborted) {
            abortSession();
            if (pendingSessionRequestIds.size > 0) {
              await supervisor.beginTermination(
                sessionGeneration,
                'cancel',
                new WhisperClientError('CANCELLED', 'Streaming transcription was cancelled.'),
                false,
                new Set(pendingSessionRequestIds),
              );
            } else if (activePushes > 0) {
              releaseDeferred = true;
              void ensureWorkerSessionCancelled().then(releaseSessionUse, releaseSessionUse);
            } else {
              await ensureWorkerSessionCancelled();
            }
          } else if (getFailureCancellation() !== null) {
            releaseDeferred = true;
          } else {
            await ensureWorkerSessionCancelled();
          }
          throw error;
        } finally {
          finishSignal?.removeEventListener('abort', abortSession);
          resolveFinishSettlement();
          if (finishState.settlement === finishSettlement) finishState.settlement = null;
          if (!releaseDeferred) releaseSessionUse();
        }
      },
      cancel: () => {
        cancellationPromise ??= (async () => {
          const existingFailureCancellation = getFailureCancellation();
          if (existingFailureCancellation !== null) {
            await existingFailureCancellation;
            return;
          }
          if (!closeSession()) {
            const finishSettlement = finishState.settlement;
            if (finishSettlement !== null) {
              sessionController.abort('streaming session cancelled during finish');
              await finishSettlement;
            }
            return;
          }
          try {
            sessionController.abort('streaming session cancelled');
            if (pendingSessionRequestIds.size > 0) {
              await supervisor.beginTermination(
                sessionGeneration,
                'cancel',
                new WhisperClientError('CANCELLED', 'Streaming transcription was cancelled.'),
                false,
                new Set(pendingSessionRequestIds),
              );
              return;
            }
            await pushTail;
            const pendingFailureCancellation = getFailureCancellation();
            if (pendingFailureCancellation !== null) await pendingFailureCancellation;
            else await ensureWorkerSessionCancelled();
          } finally {
            releaseSessionUse();
          }
        })();
        return cancellationPromise;
      },
    };
  } catch (error: unknown) {
    supervisor.releaseUseWhenSafe(sessionGeneration, () => use.release());
    throw error;
  }
}
