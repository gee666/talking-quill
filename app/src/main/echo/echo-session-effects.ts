import type { VoiceCommandMatch } from '../../shared/schemas/commands';
import { type PiFallbackCategory } from '../../shared/schemas/echo-session';
import { ProviderError } from '../providers/errors';
import { raceWithAbort } from './echo-operation';
import { type EchoSessionContext } from './echo-session-context';
import { abortSession, operationSignal } from './echo-session-operation';
import { teardown } from './echo-session-teardown';
import { playSound } from './echo-session-presentation';
import { publicSessionError } from './session-errors';
import type { SessionOutcomeWriter } from './session-outcome-writer';
import { type EchoSessionEffect, type EchoSessionState } from './session-reducer';

const INSERTION_CONTROLLER_TIMEOUT_MS = 5_000;

export function startSmartPreparation(context: EchoSessionContext, state: EchoSessionState): void {
  const session = context.outcomes.smartSession;
  if (
    state.processingMode !== 'smart' ||
    state.sessionId === null ||
    session === null ||
    context.abort === null
  ) {
    return;
  }
  if (session.prepareForListening === undefined) return;
  const captureGeneration = context.capture.generation;
  const promise = raceWithAbort(
    session.prepareForListening(context.abort.signal),
    context.abort.signal,
  );
  context.smartPreparation = {
    sessionId: state.sessionId,
    captureGeneration,
    session,
    promise,
  };
  // Preparation is intentionally speculative. Submission or teardown owns its result, while this
  // observer prevents cancellation/failure before submission from becoming an unhandled rejection.
  void promise.catch(() => undefined);
}

function preparedSmartSession(
  context: EchoSessionContext,
  session: NonNullable<SessionOutcomeWriter['smartSession']>,
): Promise<void> {
  const preparation = context.smartPreparation;
  const signal = operationSignal(context);
  const submitted = raceWithAbort(session.prepare(signal), signal);
  if (
    preparation !== null &&
    preparation.session === session &&
    preparation.sessionId === context.state.sessionId &&
    preparation.captureGeneration === context.capture.generation
  ) {
    return Promise.all([preparation.promise, submitted]).then(() => undefined);
  }
  return submitted;
}

export function enqueueEffect(context: EchoSessionContext, effect: EchoSessionEffect): void {
  const operation = async () => {
    const operationSignal = context.abort?.signal ?? null;
    try {
      await runEffect(context, effect);
    } catch (error: unknown) {
      const terminal =
        context.disposed ||
        context.state.phase === 'idle' ||
        context.state.phase === 'completed' ||
        context.state.phase === 'cancelled' ||
        context.state.phase === 'error';
      if (
        error instanceof Error &&
        error.name === 'AbortError' &&
        (operationSignal?.aborted === true || terminal)
      ) {
        if (!terminal && effect.type === 'insert') {
          context.dispatch({ type: 'insertion-cancelled' });
        }
        return;
      }
      if (!terminal) {
        context.abort?.abort();
        context.dispatch({ type: 'fail', message: publicSessionError(error) });
      }
    }
  };
  context.effectTail = context.effectTail.then(operation, operation);
}

async function runEffect(context: EchoSessionContext, effect: EchoSessionEffect): Promise<void> {
  if (effect.type === 'start-capture') {
    await context.capture.startCapture();
    return;
  }
  if (effect.type === 'begin-extended-transcription') {
    await context.capture.beginExtendedTranscription();
    return;
  }
  if (effect.type === 'stop-and-transcribe') {
    await context.capture.stopCapture();
    const signal = operationSignal(context);
    const smartSession = context.outcomes.smartSession;
    const smartPreparation =
      context.state.processingMode === 'smart' && smartSession !== null
        ? preparedSmartSession(context, smartSession)
        : Promise.resolve();
    void smartPreparation.catch(() => undefined);
    // Smart provider preparation starts in arming and submit-time context preparation overlaps
    // local inference. The local transcript remains authoritative for command bypass.
    const text = await raceWithAbort(context.capture.transcribe(), signal);
    const match: VoiceCommandMatch | null = context.commands?.match(text) ?? null;
    // In Smart mode, only an exact local match bypasses the monitor. Fuzzy and cross-language
    // candidates must be reviewed by Smart processing before they can execute.
    const executeImmediately =
      match !== null && (context.state.processingMode !== 'smart' || match.kind === 'exact');
    if (executeImmediately) {
      // Do not wait for speculative readiness before executing an exact local command.
      context.outcomes.discardSmartSession();
      context.outcomes.setVoiceCommand(match.command);
      context.dispatch({
        type: 'voice-command-matched',
        transcript: text,
        command: match.command,
      });
    } else {
      await smartPreparation;
      context.dispatch({
        type: 'transcribed',
        text,
        smart: context.state.processingMode === 'smart',
      });
    }
    return;
  }
  if (effect.type === 'process-smart') {
    const smartSession = context.outcomes.smartSession;
    if (smartSession === null) {
      context.dispatch({ type: 'abort', reason: 'provider-error' });
      return;
    }
    const signal = operationSignal(context);
    try {
      const result = await raceWithAbort(smartSession.process(effect.text, signal), signal);
      if (result.voiceCommand !== undefined && result.voiceCommand !== null) {
        context.outcomes.discardSmartSession();
        context.outcomes.setVoiceCommand(result.voiceCommand);
        context.dispatch({
          type: 'voice-command-matched',
          transcript: effect.text,
          command: result.voiceCommand,
        });
      } else {
        context.outcomes.setScreenshotFilename(result.screenshotFilename);
        context.dispatch({ type: 'smart-completed', text: result.text });
      }
    } catch (error: unknown) {
      const providerId = smartSession.providerId;
      context.outcomes.discardSmartSession();
      if (!signal.aborted && context.state.phase === 'processingSmart') {
        const reason =
          error instanceof ProviderError && error.code === 'TIMEOUT' ? 'timeout' : 'provider-error';
        abortSession(context, reason, piFallbackCategory(providerId, error));
      }
    }
    return;
  }
  if (effect.type === 'insert') {
    const signal = operationSignal(context);
    const insertionAbort = new AbortController();
    const abortInsertion = () => insertionAbort.abort(signal.reason);
    if (signal.aborted) abortInsertion();
    else signal.addEventListener('abort', abortInsertion, { once: true });
    let acceptsCommit = true;
    try {
      const result = await withDeadline(
        context.insertion.insert(
          effect.text,
          context.capture.nativeCaptureLost
            ? { ...effect.activationContext, targetToken: null }
            : effect.activationContext,
          insertionAbort.signal,
          () => {
            if (acceptsCommit) context.dispatch({ type: 'insertion-committed' });
          },
        ),
        INSERTION_CONTROLLER_TIMEOUT_MS,
        () => insertionAbort.abort(new Error('Insertion controller deadline exceeded')),
      );
      acceptsCommit = false;
      if (result.cancelled === true) context.dispatch({ type: 'insertion-cancelled' });
      else
        context.dispatch({
          type: 'inserted',
          copied: result.copied,
          ...(result.indeterminate === true ? { indeterminate: true } : {}),
        });
    } catch {
      acceptsCommit = false;
      if (context.state.phase === 'restoringClipboard') {
        context.dispatch({ type: 'inserted', copied: false });
      } else if (signal.aborted || context.state.insertionState === 'cancel-requested') {
        context.dispatch({ type: 'insertion-cancelled' });
      } else {
        // The production insertion service leaves the requested text on the clipboard when
        // native paste cannot be confirmed. Bound custom/failed ports to the same safe result.
        context.dispatch({ type: 'inserted', copied: true });
      }
    } finally {
      signal.removeEventListener('abort', abortInsertion);
    }
    return;
  }
  await teardown(context);
  if (context.state.phase === 'completed') playSound(context);
}

function piFallbackCategory(providerId: string, error: unknown): PiFallbackCategory | undefined {
  if (providerId !== 'pi' || !(error instanceof ProviderError)) return undefined;
  switch (error.code) {
    case 'UNAVAILABLE':
      return 'pi-unavailable';
    case 'AUTHENTICATION_FAILED':
      return 'pi-authentication-failed';
    case 'MODEL_NOT_FOUND':
      return 'pi-model-not-found';
    case 'NO_MODELS':
      return 'pi-no-models';
    case 'TIMEOUT':
      return 'pi-timeout';
    case 'INVALID_RESPONSE':
    case 'RESPONSE_TOO_LARGE':
      return 'pi-invalid-response';
    default:
      return 'pi-remote-failure';
  }
}

function withDeadline<Value>(
  operation: Promise<Value>,
  timeoutMs: number,
  onTimeout: () => void,
): Promise<Value> {
  return new Promise<Value>((resolve, reject) => {
    const timer = setTimeout(() => {
      onTimeout();
      reject(new Error('Insertion did not settle in time'));
    }, timeoutMs);
    timer.unref();
    operation.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error('Insertion failed'));
      },
    );
  });
}
