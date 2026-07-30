import { ECHO_HOLD_THRESHOLD_MS } from '../../shared/constants/echo-session';
import type { ProcessingMode } from '../../shared/schemas/history';
import type { VoiceCommand } from '../../shared/schemas/commands';
import type {
  EchoAbortReason,
  EchoSessionPhase,
  EchoSessionSnapshot,
  PiFallbackCategory,
} from '../../shared/schemas/echo-session';

export interface EchoSessionState extends EchoSessionSnapshot {
  readonly startedAt: number | null;
  readonly finalText: string | null;
  readonly insertionState: 'none' | 'pending' | 'cancel-requested' | 'committed';
  readonly fallbackReason: 'provider-error' | 'timeout' | null;
  readonly captureReady: boolean;
  readonly audioReady: boolean;
  readonly submitPending: boolean;
}

export type EchoSessionEvent =
  | {
      readonly type: 'shortcut-down';
      readonly sessionId: string;
      readonly alternate: boolean;
      readonly processingMode: ProcessingMode;
      readonly now: number;
    }
  | { readonly type: 'hold-elapsed'; readonly now: number }
  | { readonly type: 'shortcut-up'; readonly now: number }
  | { readonly type: 'capture-started' }
  | { readonly type: 'audio-started' }
  | { readonly type: 'level'; readonly rms: number; readonly elapsedMs: number }
  | {
      readonly type: 'submit';
      readonly source: 'silence' | 'enter' | 'shortcut' | 'stop' | 'duration-cap';
    }
  | { readonly type: 'transcribed'; readonly text: string; readonly smart: boolean }
  | {
      readonly type: 'voice-command-matched';
      readonly transcript: string;
      readonly command: VoiceCommand;
    }
  | { readonly type: 'smart-completed'; readonly text: string }
  | {
      readonly type: 'abort';
      readonly reason: EchoAbortReason;
      readonly fallbackCategory?: PiFallbackCategory;
    }
  | { readonly type: 'insertion-committed' }
  | { readonly type: 'insertion-cancelled' }
  | { readonly type: 'inserted'; readonly copied: boolean }
  | { readonly type: 'fail'; readonly message: string; readonly transcript?: string }
  | { readonly type: 'operational-failure'; readonly message: string }
  | { readonly type: 'reset' };

export type EchoSessionEffect =
  | { readonly type: 'start-capture' }
  | { readonly type: 'begin-extended-transcription' }
  | { readonly type: 'stop-and-transcribe' }
  | { readonly type: 'process-smart'; readonly text: string }
  | { readonly type: 'insert'; readonly text: string }
  | { readonly type: 'teardown' };

export interface EchoTransition {
  readonly state: EchoSessionState;
  readonly effects: readonly EchoSessionEffect[];
}

export const IDLE_ECHO_SESSION: EchoSessionState = Object.freeze({
  sessionId: null,
  phase: 'idle',
  dictationMode: null,
  processingMode: null,
  alternate: false,
  rms: 0,
  elapsedMs: 0,
  transcript: null,
  abortReason: null,
  fallbackCategory: null,
  completion: null,
  message: null,
  startedAt: null,
  finalText: null,
  insertionState: 'none',
  fallbackReason: null,
  captureReady: false,
  audioReady: false,
  submitPending: false,
});

export function reduceEchoSession(
  state: EchoSessionState,
  event: EchoSessionEvent,
): EchoTransition {
  if (event.type === 'reset') return transition(IDLE_ECHO_SESSION);
  if (event.type === 'operational-failure') {
    if (state.phase !== 'idle') return transition(state);
    return transition({ ...IDLE_ECHO_SESSION, phase: 'error', message: event.message });
  }
  if (event.type === 'shortcut-down') {
    if (state.phase !== 'idle') {
      if (isRecording(state.phase))
        return reduceEchoSession(state, { type: 'submit', source: 'shortcut' });
      return transition(state);
    }
    return transition(
      {
        ...IDLE_ECHO_SESSION,
        sessionId: event.sessionId,
        phase: 'arming',
        processingMode: event.processingMode,
        alternate: event.alternate,
        startedAt: event.now,
      },
      { type: 'start-capture' },
    );
  }
  if (event.type === 'capture-started' || event.type === 'audio-started') {
    if (!isRecordingOrArming(state.phase)) return transition(state);
    const ready = {
      ...state,
      captureReady: state.captureReady || event.type === 'capture-started',
      audioReady: state.audioReady || event.type === 'audio-started',
    };
    if (!ready.submitPending || !ready.captureReady || !ready.audioReady) {
      return transition(ready);
    }
    return transition(
      { ...ready, phase: 'transcribing', rms: 0, submitPending: false },
      { type: 'stop-and-transcribe' },
    );
  }
  if (event.type === 'hold-elapsed') {
    if (state.phase !== 'arming' || state.submitPending) return transition(state);
    return transition(
      withElapsed({ ...state, phase: 'recordingExtended', dictationMode: 'extended' }, event.now),
      { type: 'begin-extended-transcription' },
    );
  }
  if (event.type === 'shortcut-up') {
    if (state.phase === 'arming' && state.submitPending) return transition(state);
    if (state.phase === 'arming') {
      const elapsed = Math.max(0, event.now - (state.startedAt ?? event.now));
      if (elapsed >= ECHO_HOLD_THRESHOLD_MS) {
        return transition(
          withElapsed(
            { ...state, phase: 'recordingExtended', dictationMode: 'extended' },
            event.now,
          ),
          { type: 'begin-extended-transcription' },
        );
      }
      return transition(
        withElapsed({ ...state, phase: 'recordingQuick', dictationMode: 'quick' }, event.now),
      );
    }
    return transition(state);
  }
  if (event.type === 'level') {
    if (!isRecordingOrArming(state.phase)) return transition(state);
    return transition({ ...state, rms: event.rms, elapsedMs: event.elapsedMs });
  }
  if (event.type === 'submit') {
    if (!isRecordingOrArming(state.phase)) return transition(state);
    const submitted = {
      ...state,
      dictationMode: state.phase === 'arming' ? ('quick' as const) : state.dictationMode,
      rms: 0,
    };
    if (!state.captureReady || !state.audioReady) {
      return transition({ ...submitted, submitPending: true });
    }
    return transition(
      { ...submitted, phase: 'transcribing', submitPending: false },
      { type: 'stop-and-transcribe' },
    );
  }
  if (event.type === 'voice-command-matched') {
    if (state.phase !== 'transcribing' && state.phase !== 'processingSmart')
      return transition(state);
    const transcript = event.transcript.trim();
    if (transcript.length === 0) return terminalError(state, 'No speech was detected.');
    return transition(
      { ...state, phase: 'inserting', transcript, insertionState: 'pending' },
      { type: 'insert', text: event.command.snippet },
    );
  }
  if (event.type === 'transcribed') {
    if (state.phase !== 'transcribing') return transition(state);
    const text = event.text.trim();
    if (text.length === 0) {
      return terminalError(state, 'No speech was detected.');
    }
    if (event.smart) {
      return transition(
        { ...state, phase: 'processingSmart', transcript: text },
        { type: 'process-smart', text },
      );
    }
    return transition(
      {
        ...state,
        phase: 'inserting',
        transcript: text,
        finalText: text,
        insertionState: 'pending',
      },
      { type: 'insert', text },
    );
  }
  if (event.type === 'smart-completed') {
    if (state.phase !== 'processingSmart') return transition(state);
    const text = event.text.trim();
    if (text.length === 0) return terminalError(state, 'Smart processing returned no text.');
    return transition(
      { ...state, phase: 'inserting', finalText: text, insertionState: 'pending' },
      { type: 'insert', text },
    );
  }
  if (event.type === 'abort') {
    if (state.phase === 'idle' || isTerminal(state.phase) || state.phase === 'restoringClipboard') {
      return transition(state);
    }
    if (state.phase === 'inserting') {
      return transition({
        ...state,
        abortReason: event.reason,
        message: 'Cancelling insertion',
        insertionState: 'cancel-requested',
      });
    }
    if (
      (event.reason === 'provider-error' || event.reason === 'timeout') &&
      state.phase === 'processingSmart' &&
      state.transcript !== null
    ) {
      return transition(
        {
          ...state,
          phase: 'inserting',
          abortReason: event.reason,
          fallbackReason: event.reason,
          fallbackCategory: event.fallbackCategory ?? null,
          message: 'Falling back to raw',
          finalText: state.transcript,
          insertionState: 'pending',
        },
        { type: 'insert', text: state.transcript },
      );
    }
    return transition(
      { ...state, phase: 'cancelled', abortReason: event.reason, rms: 0 },
      { type: 'teardown' },
    );
  }
  if (event.type === 'insertion-committed') {
    if (state.phase !== 'inserting' || state.insertionState === 'committed')
      return transition(state);
    return transition({
      ...state,
      phase: 'restoringClipboard',
      abortReason:
        state.abortReason === 'provider-error' || state.abortReason === 'timeout'
          ? state.abortReason
          : null,
      message: null,
      insertionState: 'committed',
    });
  }
  if (event.type === 'insertion-cancelled') {
    if (state.phase !== 'inserting' || state.insertionState !== 'cancel-requested') {
      return transition(state);
    }
    return transition({ ...state, phase: 'cancelled', rms: 0 }, { type: 'teardown' });
  }
  if (event.type === 'inserted') {
    if (state.phase !== 'inserting' && state.phase !== 'restoringClipboard')
      return transition(state);
    return transition(
      {
        ...state,
        phase: 'completed',
        completion: event.copied ? 'copied' : 'inserted',
        message: event.copied ? 'Copied to clipboard instead' : 'Inserted',
        insertionState: event.copied ? 'none' : 'committed',
      },
      { type: 'teardown' },
    );
  }
  if (state.phase === 'idle' || isTerminal(state.phase)) return transition(state);
  return terminalError(state, event.message, event.transcript);
}

function terminalError(
  state: EchoSessionState,
  message: string,
  transcript?: string,
): EchoTransition {
  return transition(
    {
      ...state,
      phase: 'error',
      rms: 0,
      message,
      transcript: transcript ?? state.transcript,
    },
    { type: 'teardown' },
  );
}

function transition(state: EchoSessionState, ...effects: EchoSessionEffect[]): EchoTransition {
  return { state, effects };
}

function withElapsed(state: EchoSessionState, now: number): EchoSessionState {
  return { ...state, elapsedMs: Math.max(0, now - (state.startedAt ?? now)) };
}

function isRecording(phase: EchoSessionPhase): boolean {
  return phase === 'recordingQuick' || phase === 'recordingExtended';
}

function isRecordingOrArming(phase: EchoSessionPhase): boolean {
  return phase === 'arming' || isRecording(phase);
}

function isTerminal(phase: EchoSessionPhase): boolean {
  return phase === 'completed' || phase === 'cancelled' || phase === 'error';
}
