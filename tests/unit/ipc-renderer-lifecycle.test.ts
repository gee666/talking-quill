import { EventEmitter } from 'node:events';
import type { WebContents } from 'electron';
import { describe, expect, it, vi } from 'vitest';
import { subscribeToRendererInvalidation } from '../../app/src/main/ipc/transport';

function source(): EventEmitter & WebContents {
  return new EventEmitter() as EventEmitter & WebContents;
}

describe('renderer lifecycle invalidation', () => {
  it.each(['destroyed', 'render-process-gone', 'did-navigate'] as const)(
    'invalidates once and removes every lifecycle listener on %s',
    (eventName) => {
      const sender = source();
      const listener = vi.fn();
      subscribeToRendererInvalidation(sender, listener);

      sender.emit(eventName);
      sender.emit(eventName);

      expect(listener).toHaveBeenCalledOnce();
      for (const event of [
        'destroyed',
        'render-process-gone',
        'did-start-navigation',
        'did-navigate',
      ]) {
        expect(sender.listenerCount(event)).toBe(0);
      }
    },
  );

  it('invalidates only document-replacing main-frame navigation starts', () => {
    const sender = source();
    const listener = vi.fn();
    subscribeToRendererInvalidation(sender, listener);

    sender.emit('did-start-navigation', {}, 'https://example.test/subframe', false, false);
    sender.emit('did-start-navigation', {}, 'https://example.test/#same-document', true, true);
    expect(listener).not.toHaveBeenCalled();

    sender.emit('did-start-navigation', {}, 'https://example.test/replaced', false, true);
    expect(listener).toHaveBeenCalledOnce();
  });

  it('supports idempotent manual cleanup', () => {
    const sender = source();
    const listener = vi.fn();
    const remove = subscribeToRendererInvalidation(sender, listener);

    remove();
    remove();
    sender.emit('destroyed');

    expect(listener).not.toHaveBeenCalled();
  });
});
