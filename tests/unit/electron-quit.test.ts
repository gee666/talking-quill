import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createBoundedElectronQuit } from '../../app/src/main/app/electron-quit';

afterEach(() => {
  vi.useRealTimers();
});

describe('bounded Electron quit', () => {
  it('forces bootstrap app.exit at the original deadline when app.quit does not end the process', async () => {
    const bootstrap = readFileSync('app/src/main/bootstrap.ts', 'utf8');
    expect(bootstrap).toContain('createBoundedElectronQuit(app, deadline');
    expect(bootstrap).not.toMatch(/\bapp\.quit\(/u);

    vi.useFakeTimers();
    const quit = vi.fn();
    const exit = vi.fn();
    const deadline = Date.now() + 100;

    await vi.advanceTimersByTimeAsync(80);
    const bounded = createBoundedElectronQuit({ quit, exit }, deadline);
    bounded.request(0);

    expect(quit).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(19);
    expect(exit).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(exit).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('uses app.exit immediately when Electron throws while requesting quit', () => {
    vi.useFakeTimers();
    const exit = vi.fn();
    const bounded = createBoundedElectronQuit(
      {
        quit: () => {
          throw new Error('quit failed');
        },
        exit,
      },
      Date.now() + 100,
    );

    bounded.request(2);

    expect(exit).toHaveBeenCalledExactlyOnceWith(2);
  });
});
