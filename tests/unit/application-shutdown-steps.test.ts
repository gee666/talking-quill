import { describe, expect, it, vi } from 'vitest';
import { createApplicationDrainSteps } from '../../app/src/main/app/shutdown-steps';

describe('application shutdown steps', () => {
  it('keeps one producer-to-store order for reset and ordinary shutdown', async () => {
    const calls: string[] = [];
    const record = (name: string): void => void calls.push(name);
    const ipc = { drain: vi.fn(() => Promise.resolve(record('ipc-drain'))) };
    const targets = {
      ipc,
      tray: { drain: vi.fn(() => Promise.resolve(record('tray-actions'))) },
      providerMutations: {
        drain: vi.fn(() => Promise.resolve(record('provider-mutations'))),
      },
      echo: { shutdown: vi.fn(() => record('echo-session')) },
      recording: { shutdown: vi.fn(() => record('recording')) },
      models: { shutdown: vi.fn(() => record('models')) },
      whisper: { close: vi.fn(() => record('whisper-worker')) },
      helper: { stop: vi.fn(() => record('helper')) },
      history: { close: vi.fn(() => record('history')) },
      settings: { flush: vi.fn(() => Promise.resolve(record('settings'))) },
      vault: { flush: vi.fn(() => Promise.resolve(record('vault'))) },
      diagnostics: { dispose: vi.fn(() => record('diagnostic-logger')) },
    };

    const steps = createApplicationDrainSteps(targets, ['data:reset-all']);
    for (const step of steps) await step.run();

    const expectedOrder = [
      'ipc-drain',
      'tray-actions',
      'provider-mutations',
      'echo-session',
      'recording',
      'models',
      'whisper-worker',
      'helper',
      'history',
      'settings',
      'vault',
      'diagnostic-logger',
    ];
    expect(steps.map(({ name }) => name)).toEqual(expectedOrder);
    expect(calls).toEqual(expectedOrder);
    expect(ipc.drain).toHaveBeenCalledWith(['data:reset-all']);
  });
});
