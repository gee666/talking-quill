import type { MessagePortMain } from 'electron';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CAPTURE_COMMAND_TIMEOUT_MS } from '../../app/src/shared/constants/audio';
import { CaptureClientError } from '../../app/src/main/audio/capture-client-error';
import { CaptureRequests } from '../../app/src/main/audio/capture-requests';

function harness() {
  const requests = new CaptureRequests();
  const postMessage = vi.fn();
  const port = { postMessage } as unknown as MessagePortMain;
  const close = vi.fn(() => requests.rejectAll());
  const command = { type: 'devices:list', requestId: 'request-1' } as const;
  return { requests, port, postMessage, close, command };
}

afterEach(() => vi.useRealTimers());

describe('CaptureRequests', () => {
  it('clears the deadline and abort listener before resolving a matching response', async () => {
    vi.useFakeTimers();
    const test = harness();
    const controller = new AbortController();
    const removeListener = vi.spyOn(controller.signal, 'removeEventListener');
    const result = test.requests.request(test.port, test.close, test.command, controller.signal);
    const response = { type: 'devices:list-result' as const, requestId: 'request-1', devices: [] };
    test.requests.settle({ ...response, requestId: 'unrelated' });
    expect(vi.getTimerCount()).toBe(1);
    test.requests.settle(response);
    expect(vi.getTimerCount()).toBe(0);
    expect(removeListener).toHaveBeenCalledOnce();
    await expect(result).resolves.toBe(response);
    controller.abort();
    expect(test.close).not.toHaveBeenCalled();
  });

  it('expires only the timed-out request without closing the port', async () => {
    vi.useFakeTimers();
    const test = harness();
    const result = test.requests.request(test.port, test.close, test.command);
    const rejected = expect(result).rejects.toMatchObject({ code: 'capture-unavailable' });
    await vi.advanceTimersByTimeAsync(CAPTURE_COMMAND_TIMEOUT_MS);
    await rejected;
    test.requests.settle({ type: 'devices:list-result', requestId: 'request-1', devices: [] });
    expect(test.close).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('closes on abort and releases every pending request through port cleanup', async () => {
    vi.useFakeTimers();
    const test = harness();
    const controller = new AbortController();
    const first = test.requests.request(test.port, test.close, test.command, controller.signal);
    const second = test.requests.request(test.port, test.close, {
      ...test.command,
      requestId: 'request-2',
    });
    const results = Promise.allSettled([first, second]);
    controller.abort();
    expect(test.close).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(await results).toEqual([
      { status: 'rejected', reason: new CaptureClientError('capture-unavailable') },
      { status: 'rejected', reason: new CaptureClientError('capture-unavailable') },
    ]);
    test.requests.rejectAll();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('preserves protocol errors and removes their abort listener', async () => {
    vi.useFakeTimers();
    const test = harness();
    const controller = new AbortController();
    const result = test.requests.request(test.port, test.close, test.command, controller.signal);
    test.requests.settle({
      type: 'request:error',
      requestId: 'request-1',
      captureId: null,
      code: 'permission-denied',
    });
    await expect(result).rejects.toMatchObject({ code: 'permission-denied' });
    controller.abort();
    expect(test.close).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('cleans up after a postMessage failure without closing the port', async () => {
    vi.useFakeTimers();
    const test = harness();
    const controller = new AbortController();
    test.postMessage.mockImplementation(() => {
      throw new Error('closed');
    });
    await expect(
      test.requests.request(test.port, test.close, test.command, controller.signal),
    ).rejects.toMatchObject({ code: 'capture-unavailable' });
    controller.abort();
    expect(test.close).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('rejects missing ports and already-aborted calls without posting or scheduling', async () => {
    vi.useFakeTimers();
    const test = harness();
    await expect(test.requests.request(null, test.close, test.command)).rejects.toMatchObject({
      code: 'capture-unavailable',
    });
    await expect(
      test.requests.request(test.port, test.close, test.command, AbortSignal.abort()),
    ).rejects.toMatchObject({ code: 'capture-unavailable' });
    expect(test.postMessage).not.toHaveBeenCalled();
    expect(test.close).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});
