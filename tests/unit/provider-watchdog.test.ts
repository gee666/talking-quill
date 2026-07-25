import { afterEach, describe, expect, it, vi } from 'vitest';
// @vitest-environment jsdom

import { rendererWatchdog } from '../../app/src/renderer/main/provider-watchdog';

describe('renderer provider watchdog', () => {
  afterEach(() => vi.useRealTimers());

  it('rejects immediately at the watchdog deadline even when cancel IPC never settles', async () => {
    vi.useFakeTimers();
    const cancel = vi.fn(() => new Promise<never>(() => undefined));
    const result = expect(
      rendererWatchdog(new Promise<never>(() => undefined), cancel),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.advanceTimersByTimeAsync(125_000);
    await result;
    expect(cancel).toHaveBeenCalledOnce();
  });
});
