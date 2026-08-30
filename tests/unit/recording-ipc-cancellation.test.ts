import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import type { AuthorizedIpcContext } from '../../app/src/main/ipc/types';

function cancellationContext() {
  let destroy: (() => void) | null = null;
  const remove = vi.fn();
  const context: AuthorizedIpcContext = {
    webContentsId: 42,
    onDestroyed(listener) {
      destroy = listener;
      return remove;
    },
  };
  return {
    context,
    destroy() {
      if (destroy === null) throw new Error('The handler did not register cancellation');
      destroy();
    },
    remove,
  };
}

function pendingUntilAborted(signal: AbortSignal) {
  return new Promise<never>((_resolve, reject) => {
    signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true });
  });
}

describe('recording IPC cancellation', () => {
  it('cancels recording:start-test when shutdown invalidates its invocation', async () => {
    const owner = { id: 42 };
    const startTest = vi.fn((_owner: unknown, signal: AbortSignal) => pendingUntilAborted(signal));
    const handlers = createHandlers({
      recording: { startTest },
      windows: { getByWebContentsId: () => ({ webContents: owner }) },
    } as unknown as HandlerDependencies);
    const cancellation = cancellationContext();

    const invocation = handlers['recording:start-test']({}, cancellation.context);
    await vi.waitFor(() => expect(startTest).toHaveBeenCalledOnce());
    const signal = startTest.mock.calls[0]?.[1];
    expect(signal?.aborted).toBe(false);

    cancellation.destroy();

    await expect(invocation).rejects.toThrow('cancelled');
    expect(signal?.aborted).toBe(true);
    expect(cancellation.remove).toHaveBeenCalledOnce();
  });

  it('cancels recording:stop-test when shutdown invalidates its invocation', async () => {
    const stopTest = vi.fn((_ownerId: number, signal: AbortSignal) => pendingUntilAborted(signal));
    const handlers = createHandlers({
      recording: { stopTest },
    } as unknown as HandlerDependencies);
    const cancellation = cancellationContext();

    const invocation = handlers['recording:stop-test']({}, cancellation.context);
    await vi.waitFor(() => expect(stopTest).toHaveBeenCalledOnce());
    expect(stopTest.mock.calls[0]?.[0]).toBe(42);

    cancellation.destroy();

    await expect(invocation).rejects.toThrow('cancelled');
    expect(stopTest.mock.calls[0]?.[1].aborted).toBe(true);
    expect(cancellation.remove).toHaveBeenCalledOnce();
  });
});
