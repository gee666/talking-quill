import { describe, expect, it } from 'vitest';
import { PiOperationScheduler } from '../../app/src/main/providers/pi-operation-scheduler';

const signal = new AbortController().signal;

describe('Pi operation scheduler', () => {
  it('lets the newest queued speculation supersede every older unused generation', async () => {
    const scheduler = new PiOperationScheduler();
    const first = await scheduler.acquireSpeculative(signal);
    const second = scheduler.acquireSpeculative(signal);
    const third = scheduler.acquireSpeculative(signal);

    first.release();
    await expect(second).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
    const newest = await third;
    newest.release();
  });

  it('revokes unused speculation for foreground work but preserves a committed turn', async () => {
    const scheduler = new PiOperationScheduler();
    const unused = await scheduler.acquireSpeculative(signal);
    const foreground = scheduler.acquireForeground(signal);
    expect(unused.revocationSignal.aborted).toBe(true);
    unused.release();
    const foregroundPermit = await foreground;
    foregroundPermit.release();

    const committed = await scheduler.acquireSpeculative(signal);
    expect(committed.commit()).toBe(true);
    const queuedForeground = scheduler.acquireForeground(signal);
    expect(committed.revocationSignal.aborted).toBe(false);
    committed.release();
    const queuedPermit = await queuedForeground;
    queuedPermit.release();
  });
});
