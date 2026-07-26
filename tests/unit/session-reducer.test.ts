import { describe, expect, it } from 'vitest';
import { IDLE_ECHO_SESSION, reduceEchoSession } from '../../app/src/main/echo/session-reducer';

const SESSION_ID = '00000000-0000-4000-8000-000000000001';

function arming() {
  return reduceEchoSession(IDLE_ECHO_SESSION, {
    type: 'shortcut-down',
    sessionId: SESSION_ID,
    alternate: false,
    processingMode: 'raw',
    now: 1_000,
  }).state;
}

function withCaptureAndAudio(state: ReturnType<typeof arming>) {
  const captured = reduceEchoSession(state, { type: 'capture-started' }).state;
  return reduceEchoSession(captured, { type: 'audio-started' }).state;
}

describe('Echo session reducer', () => {
  it('starts exactly one session and ignores a competing shortcut', () => {
    const first = reduceEchoSession(IDLE_ECHO_SESSION, {
      type: 'shortcut-down',
      sessionId: SESSION_ID,
      alternate: false,
      processingMode: 'raw',
      now: 10,
    });
    expect(first.state).toMatchObject({ phase: 'arming', sessionId: SESSION_ID });
    expect(first.effects).toEqual([{ type: 'start-capture' }]);
    expect(
      reduceEchoSession(first.state, {
        type: 'shortcut-down',
        sessionId: '00000000-0000-4000-8000-000000000002',
        alternate: true,
        processingMode: 'smart',
        now: 11,
      }).state.sessionId,
    ).toBe(SESSION_ID);
  });

  it('classifies release before the hold timer as Quick', () => {
    const result = reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_599 });
    expect(result.state).toMatchObject({
      phase: 'recordingQuick',
      dictationMode: 'quick',
      elapsedMs: 599,
    });
  });

  it('classifies a threshold release as Extended even when the hold timer was delayed', () => {
    const result = reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_600 });
    expect(result.state).toMatchObject({
      phase: 'recordingExtended',
      dictationMode: 'extended',
      elapsedMs: 600,
    });
    expect(result.effects).toEqual([{ type: 'begin-extended-transcription' }]);
  });

  it('classifies the hold threshold as Extended and release does not submit', () => {
    const held = reduceEchoSession(arming(), { type: 'hold-elapsed', now: 1_600 });
    expect(held.state).toMatchObject({
      phase: 'recordingExtended',
      dictationMode: 'extended',
      elapsedMs: 600,
    });
    expect(held.effects).toEqual([{ type: 'begin-extended-transcription' }]);
    expect(reduceEchoSession(held.state, { type: 'shortcut-up', now: 1_601 }).state.phase).toBe(
      'recordingExtended',
    );
  });

  it.each(['silence', 'enter', 'shortcut', 'stop', 'duration-cap'] as const)(
    'defers an arming submit from %s until capture has started',
    (source) => {
      const pending = reduceEchoSession(arming(), { type: 'submit', source });
      expect(pending.state).toMatchObject({
        phase: 'arming',
        dictationMode: 'quick',
        submitPending: true,
      });
      expect(pending.effects).toEqual([]);

      const captured = reduceEchoSession(pending.state, { type: 'capture-started' });
      expect(captured.state).toMatchObject({ phase: 'arming', captureReady: true });
      expect(captured.effects).toEqual([]);

      const result = reduceEchoSession(captured.state, { type: 'audio-started' });
      expect(result.state).toMatchObject({ phase: 'transcribing', submitPending: false });
      expect(result.effects).toEqual([{ type: 'stop-and-transcribe' }]);
    },
  );

  it('defers a Quick submit when release wins the capture-start race', () => {
    const quick = reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_100 }).state;
    const pending = reduceEchoSession(quick, { type: 'submit', source: 'enter' });
    expect(pending.state).toMatchObject({ phase: 'recordingQuick', submitPending: true });
    expect(pending.effects).toEqual([]);

    const captured = reduceEchoSession(pending.state, { type: 'capture-started' }).state;
    const result = reduceEchoSession(captured, { type: 'audio-started' });
    expect(result.state.phase).toBe('transcribing');
    expect(result.effects).toEqual([{ type: 'stop-and-transcribe' }]);
  });

  it.each(['silence', 'enter', 'shortcut', 'stop', 'duration-cap'] as const)(
    'submits a recording from %s',
    (source) => {
      let quick = reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_100 }).state;
      quick = reduceEchoSession(quick, { type: 'capture-started' }).state;
      quick = reduceEchoSession(quick, { type: 'audio-started' }).state;
      const result = reduceEchoSession(quick, { type: 'submit', source });
      expect(result.state.phase).toBe('transcribing');
      expect(result.effects).toEqual([{ type: 'stop-and-transcribe' }]);
    },
  );

  it('routes raw transcription directly to insertion', () => {
    const quick = withCaptureAndAudio(
      reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_100 }).state,
    );
    const transcribing = reduceEchoSession(quick, { type: 'submit', source: 'enter' }).state;
    const result = reduceEchoSession(transcribing, {
      type: 'transcribed',
      text: ' hello ',
      smart: false,
    });
    expect(result.state).toMatchObject({ phase: 'inserting', transcript: 'hello' });
    expect(result.effects).toEqual([{ type: 'insert', text: 'hello' }]);
  });

  it('supports the Smart extension point and raw fallback', () => {
    const quick = withCaptureAndAudio(
      reduceEchoSession(arming(), { type: 'shortcut-up', now: 1_100 }).state,
    );
    const transcribing = reduceEchoSession(quick, { type: 'submit', source: 'enter' }).state;
    const smart = reduceEchoSession(transcribing, {
      type: 'transcribed',
      text: 'raw',
      smart: true,
    });
    expect(smart.state.phase).toBe('processingSmart');
    const fallback = reduceEchoSession(smart.state, { type: 'abort', reason: 'provider-error' });
    expect(fallback.state).toMatchObject({ phase: 'inserting', abortReason: 'provider-error' });
    expect(fallback.effects).toEqual([{ type: 'insert', text: 'raw' }]);
  });

  it.each(['user-cancel', 'shutdown', 'target-lost'] as const)(
    'makes terminal abort %s cancel without insertion',
    (reason) => {
      const result = reduceEchoSession(arming(), { type: 'abort', reason });
      expect(result.state).toMatchObject({ phase: 'cancelled', abortReason: reason });
      expect(result.effects).toEqual([{ type: 'teardown' }]);
    },
  );

  it.each([
    'arming',
    'recordingQuick',
    'recordingExtended',
    'transcribing',
    'processingSmart',
  ] as const)('tears every active phase down on shutdown: %s', (phase) => {
    const state = {
      ...arming(),
      phase,
      dictationMode: phase === 'recordingExtended' ? ('extended' as const) : ('quick' as const),
      transcript: phase === 'processingSmart' ? 'usable' : null,
    };
    const result = reduceEchoSession(state, { type: 'abort', reason: 'shutdown' });
    expect(result.state).toMatchObject({ phase: 'cancelled', abortReason: 'shutdown' });
    expect(result.effects).toEqual([{ type: 'teardown' }]);
  });

  it('keeps shutdown commit-pending until paste dispatch resolves', () => {
    const inserting = {
      ...arming(),
      phase: 'inserting' as const,
      dictationMode: 'quick' as const,
      transcript: 'usable',
      insertionState: 'pending' as const,
    };
    const pending = reduceEchoSession(inserting, { type: 'abort', reason: 'shutdown' });
    expect(pending.state).toMatchObject({
      phase: 'inserting',
      abortReason: 'shutdown',
      insertionState: 'cancel-requested',
    });
    expect(pending.effects).toEqual([]);
    const cancelled = reduceEchoSession(pending.state, { type: 'insertion-cancelled' });
    expect(cancelled.state).toMatchObject({ phase: 'cancelled', abortReason: 'shutdown' });
    expect(cancelled.effects).toEqual([{ type: 'teardown' }]);
  });

  it('lets a committed paste win over a pending shutdown without reporting cancellation', () => {
    const pending = {
      ...arming(),
      phase: 'inserting' as const,
      dictationMode: 'quick' as const,
      transcript: 'usable',
      abortReason: 'shutdown' as const,
      insertionState: 'cancel-requested' as const,
    };
    const committed = reduceEchoSession(pending, { type: 'insertion-committed' });
    expect(committed.state).toMatchObject({
      phase: 'restoringClipboard',
      abortReason: null,
      insertionState: 'committed',
    });
    expect(reduceEchoSession(committed.state, { type: 'abort', reason: 'shutdown' }).state).toBe(
      committed.state,
    );
  });

  it.each(['provider-error', 'timeout'] as const)(
    'supports nonterminal Smart abort %s',
    (reason) => {
      const processing = {
        ...arming(),
        phase: 'processingSmart' as const,
        transcript: 'usable raw',
      };
      expect(reduceEchoSession(processing, { type: 'abort', reason }).effects).toContainEqual({
        type: 'insert',
        text: 'usable raw',
      });
    },
  );

  it('reports empty transcription as an error with guaranteed teardown', () => {
    const transcribing = {
      ...arming(),
      phase: 'transcribing' as const,
      dictationMode: 'quick' as const,
    };
    const result = reduceEchoSession(transcribing, {
      type: 'transcribed',
      text: '  ',
      smart: false,
    });
    expect(result.state).toMatchObject({ phase: 'error', message: 'No speech was detected.' });
    expect(result.effects).toEqual([{ type: 'teardown' }]);
  });

  it.each(['user-cancel', 'shutdown', 'target-lost'] as const)(
    'keeps committed insertion authoritative across late %s',
    (reason) => {
      const inserting = { ...arming(), phase: 'inserting' as const, transcript: 'hello' };
      const committed = reduceEchoSession(inserting, { type: 'insertion-committed' }).state;
      expect(committed.phase).toBe('restoringClipboard');
      expect(reduceEchoSession(committed, { type: 'abort', reason }).state).toBe(committed);
      expect(reduceEchoSession(committed, { type: 'inserted', copied: false }).state).toMatchObject(
        {
          phase: 'completed',
          completion: 'inserted',
        },
      );
    },
  );

  it('marks clipboard fallback distinctly and resets to idle', () => {
    const inserting = { ...arming(), phase: 'inserting' as const, transcript: 'hello' };
    const completed = reduceEchoSession(inserting, { type: 'inserted', copied: true });
    expect(completed.state).toMatchObject({ phase: 'completed', completion: 'copied' });
    expect(reduceEchoSession(completed.state, { type: 'reset' }).state).toEqual(IDLE_ECHO_SESSION);
  });

  it.each([
    {
      name: 'smart-completed inserts non-empty output',
      state: { ...arming(), phase: 'processingSmart' as const, transcript: 'raw' },
      event: { type: 'smart-completed' as const, text: ' polished ' },
      phase: 'inserting',
      effects: [{ type: 'insert', text: 'polished' }],
    },
    {
      name: 'smart-completed rejects empty output',
      state: { ...arming(), phase: 'processingSmart' as const, transcript: 'raw' },
      event: { type: 'smart-completed' as const, text: '   ' },
      phase: 'error',
      effects: [{ type: 'teardown' }],
    },
    {
      name: 'fail preserves a usable transcript',
      state: { ...arming(), phase: 'transcribing' as const },
      event: { type: 'fail' as const, message: 'worker failed', transcript: 'partial' },
      phase: 'error',
      effects: [{ type: 'teardown' }],
    },
    {
      name: 'inserted false completes as inserted',
      state: { ...arming(), phase: 'inserting' as const, transcript: 'text' },
      event: { type: 'inserted' as const, copied: false },
      phase: 'completed',
      effects: [{ type: 'teardown' }],
    },
    {
      name: 'reset always returns idle',
      state: { ...arming(), phase: 'completed' as const },
      event: { type: 'reset' as const },
      phase: 'idle',
      effects: [],
    },
    {
      name: 'invalid submit while idle is ignored',
      state: IDLE_ECHO_SESSION,
      event: { type: 'submit' as const, source: 'enter' as const },
      phase: 'idle',
      effects: [],
    },
    {
      name: 'stale transcription while recording is ignored',
      state: { ...arming(), phase: 'recordingQuick' as const, dictationMode: 'quick' as const },
      event: { type: 'transcribed' as const, text: 'stale', smart: false },
      phase: 'recordingQuick',
      effects: [],
    },
    {
      name: 'terminal fail is ignored',
      state: { ...arming(), phase: 'cancelled' as const },
      event: { type: 'fail' as const, message: 'late failure' },
      phase: 'cancelled',
      effects: [],
    },
  ])('$name', ({ state, event, phase, effects }) => {
    const result = reduceEchoSession(state, event);
    expect(result.state.phase).toBe(phase);
    expect(result.effects).toEqual(effects);
    if (event.type === 'fail' && 'transcript' in event) {
      expect(result.state.transcript).toBe('partial');
    }
    if (event.type === 'inserted') expect(result.state.completion).toBe('inserted');
  });
});
