import { describe, expect, it, vi } from 'vitest';
import { UpdateOperationCoordinator } from '../../app/src/main/info/update-operation-coordinator';

function owner(id: number) {
  let destroyed: (() => void) | null = null;
  return {
    value: {
      webContentsId: id,
      onDestroyed(listener: () => void) {
        destroyed = listener;
        return () => {
          destroyed = null;
        };
      },
    },
    destroy: () => destroyed?.(),
  };
}

describe('UpdateOperationCoordinator', () => {
  it('allows only one active operation per owner without affecting another owner', async () => {
    const coordinator = new UpdateOperationCoordinator();
    const first = owner(1);
    let release!: () => void;
    const pending = coordinator.run(
      first.value,
      'same',
      (signal) =>
        new Promise<void>((resolve) => {
          release = resolve;
          signal.addEventListener('abort', () => resolve(), { once: true });
        }),
    );
    await expect(coordinator.run(first.value, 'same', () => Promise.resolve())).rejects.toThrow(
      'update operation',
    );
    await expect(
      coordinator.run(first.value, 'different', () => Promise.resolve()),
    ).rejects.toThrow('update operation');
    await expect(
      coordinator.run(owner(2).value, 'same', () => Promise.resolve('ok')),
    ).resolves.toBe('ok');
    release();
    await pending;
    await expect(
      coordinator.run(first.value, 'after-settlement', () => Promise.resolve('reused')),
    ).resolves.toBe('reused');
  });

  it('isolates cancellation and aborts on owner destruction and shutdown', async () => {
    const coordinator = new UpdateOperationCoordinator();
    const first = owner(1);
    const second = owner(2);
    const aborted = vi.fn();
    const run = (candidate: ReturnType<typeof owner>) =>
      coordinator.run(
        candidate.value,
        'check',
        (signal) =>
          new Promise<void>((resolve) => {
            signal.addEventListener('abort', () => {
              aborted(candidate.value.webContentsId);
              resolve();
            });
          }),
      );
    const a = run(first);
    const b = run(second);
    expect(coordinator.cancel(1, 'check')).toBe(true);
    await a;
    expect(aborted).toHaveBeenCalledWith(1);
    expect(aborted).not.toHaveBeenCalledWith(2);
    second.destroy();
    await b;

    const third = owner(3);
    const c = run(third);
    coordinator.dispose();
    await c;
    await expect(coordinator.run(third.value, 'new', () => Promise.resolve())).rejects.toThrow(
      'shutting down',
    );
  });
});
