import { describe, expect, it, vi } from 'vitest';
import { AppStateService } from '../../app/src/main/app/app-state-service';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

describe('AppStateService readiness', () => {
  it('reports disabled ahead of otherwise-ready helper/model state', () => {
    let settings: Settings = {
      ...structuredClone(DEFAULT_SETTINGS),
      app: { ...DEFAULT_SETTINGS.app, enabled: false },
    };
    const listeners = new Set<(next: Settings) => void>();
    const store = {
      get: () => settings,
      subscribe: (listener: (next: Settings) => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      update: vi.fn(),
    } as unknown as SettingsStore;
    const service = new AppStateService(store, { send: vi.fn() } as unknown as IpcEventEmitter);
    service.setHelperReadiness({
      status: 'ready',
      reason: null,
      helperVersion: '1.0.0',
      permissions: {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      },
    });
    service.setModelReady(true);
    expect(service.getState()).toMatchObject({
      enabled: false,
      status: 'disabled',
      modelReady: true,
    });
    service.setModelReady(false);
    expect(service.getState()).toMatchObject({ status: 'disabled', modelReady: false });
    service.setModelReady(true);

    settings = { ...settings, app: { ...settings.app, enabled: true } };
    for (const listener of listeners) listener(settings);
    expect(service.getState().status).toBe('ready');
  });
});
