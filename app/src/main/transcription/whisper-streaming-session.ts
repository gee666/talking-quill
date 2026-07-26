import { randomUUID } from 'node:crypto';
import {
  WHISPER_CHUNK_SECONDS,
  WHISPER_HOP_SECONDS,
  WHISPER_MAX_PUSH_SAMPLES,
  WHISPER_MAX_SAMPLES,
  WHISPER_PROTOCOL_VERSION,
  WHISPER_SAMPLE_RATE,
} from '../../shared/constants/whisper';
import type { TranscriptionOptions, TranscriptionResult } from '../../shared/schemas/transcription';
import type {
  WhisperAcknowledgedOperation,
  WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';
import { CONTROL_REQUEST_TIMEOUT_MS } from './whisper-worker-supervisor';
import type { WhisperWorkerSupervisor } from './whisper-worker-supervisor';

const INFERENCE_STARTUP_TIMEOUT_MS = 5 * 60_000;
const INFERENCE_REALTIME_MULTIPLIER = 3;
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
              accepts: isTranscriptionResult,
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

export function inferenceTimeoutMs(sampleCount: number): number {
  const audioDurationMs = Math.ceil((sampleCount * 1_000) / WHISPER_SAMPLE_RATE);
  return INFERENCE_STARTUP_TIMEOUT_MS + audioDurationMs * INFERENCE_REALTIME_MULTIPLIER;
}

function acceptsAcknowledgement(
  operation: WhisperAcknowledgedOperation,
): (result: WhisperWorkerResult) => boolean {
  return (result) => result.type === 'acknowledged' && result.operation === operation;
}

function isTranscriptionResult(result: WhisperWorkerResult): boolean {
  return result.type === 'transcription';
}

function assertAcknowledged(
  result: WhisperWorkerResult,
  operation: WhisperAcknowledgedOperation,
): void {
  if (result.type !== 'acknowledged' || result.operation !== operation) {
    throw new WhisperClientError(
      'PROTOCOL_ERROR',
      `Whisper worker returned the wrong acknowledgement for ${operation}.`,
    );
  }
}

function combineAbortSignals(first: AbortSignal | undefined, second: AbortSignal): AbortSignal {
  return first === undefined ? second : AbortSignal.any([first, second]);
}

function streamingPushPlan(
  bufferedSamples: number,
  pushedSamples: number,
): {
  readonly remainingSamples: number;
  readonly timeoutMs: number;
} {
  const chunkSamples = WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SECONDS;
  const hopSamples = WHISPER_SAMPLE_RATE * WHISPER_HOP_SECONDS;
  let remainingSamples = bufferedSamples + pushedSamples;
  let inferenceSamples = 0;
  while (remainingSamples >= chunkSamples) {
    inferenceSamples += chunkSamples;
    remainingSamples -= hopSamples;
  }
  return {
    remainingSamples,
    timeoutMs:
      inferenceSamples === 0 ? CONTROL_REQUEST_TIMEOUT_MS : inferenceTimeoutMs(inferenceSamples),
  };
}

function waitForDispatchTurn(
  precedingRequest: Promise<void>,
  signal: AbortSignal | undefined,
): Promise<void> {
  if (signal?.aborted === true) {
    return Promise.reject(new WhisperClientError('CANCELLED', 'Transcription was cancelled.'));
  }
  if (signal === undefined) return precedingRequest;
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (operation: () => void): void => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      operation();
    };
    const abort = (): void =>
      finish(() => reject(new WhisperClientError('CANCELLED', 'Transcription was cancelled.')));
    signal.addEventListener('abort', abort, { once: true });
    void precedingRequest.then(
      () => finish(resolve),
      () => finish(resolve),
    );
  });
}

function assertPcmLength(pcm: Float32Array, maximum: number, message: string): void {
  if (pcm.length === 0 || pcm.length > maximum) {
    throw new WhisperClientError('INVALID_AUDIO', message);
  }
}

function copyPcm(pcm: Float32Array): ArrayBuffer {
  const copy = new Float32Array(pcm.length);
  copy.set(pcm);
  return copy.buffer;
}

function once(operation: () => void): () => void {
  let called = false;
  return () => {
    if (called) return;
    called = true;
    operation();
  };
}
