import { describe, expect, it, vi } from 'vitest';
import { subscribeToWidgetSession } from '../../app/src/renderer/widget/session-subscription';
import type { EchoSessionSnapshot } from '../../app/src/shared/schemas/echo-session';

const IDLE: EchoSessionSnapshot = {
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
};

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe('widget session subscription', () => {
  it('does not let a stale ready snapshot overwrite a pushed update', async () => {
    const ready = deferred<EchoSessionSnapshot>();
    const push: { current: ((snapshot: EchoSessionSnapshot) => void) | null } = { current: null };
    const listener = vi.fn();
    const unsubscribe = vi.fn();

    const dispose = subscribeToWidgetSession(
      {
        ready: () => ready.promise,
        onSessionChanged: (next) => {
          push.current = next;
          return unsubscribe;
        },
      },
      listener,
    );
    const recording: EchoSessionSnapshot = {
      ...IDLE,
      sessionId: '00000000-0000-4000-8000-000000000001',
      phase: 'recordingQuick',
      dictationMode: 'quick',
      processingMode: 'raw',
    };
    push.current?.(recording);
    ready.resolve(IDLE);
    await ready.promise;
    await Promise.resolve();

    expect(listener).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith(recording);
    dispose();
    expect(unsubscribe).toHaveBeenCalledOnce();
  });
});
