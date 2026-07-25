import { describe, expect, it, vi } from 'vitest';
import { AppStateService } from '../../app/src/main/app/app-state-service';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { EchoSessionSnapshot } from '../../app/src/shared/schemas/echo-session';
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

  it('caches enabled state and suppresses equivalent public state events', () => {
    let settings = structuredClone(DEFAULT_SETTINGS);
    const listeners = new Set<(next: Settings) => void>();
    const get = vi.fn(() => settings);
    const send = vi.fn();
    const store = {
      get,
      subscribe: (listener: (next: Settings) => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      update: vi.fn(),
    } as unknown as SettingsStore;
    const service = new AppStateService(store, { send } as unknown as IpcEventEmitter);

    expect(get).toHaveBeenCalledOnce();
    service.getState();
    service.getState();
    service.setModelReady(false);
    service.setSession({ phase: 'arming' } as EchoSessionSnapshot);
    service.setSession({ phase: 'recordingQuick' } as EchoSessionSnapshot);
    service.setSession({ phase: 'recordingQuick' } as EchoSessionSnapshot);
    expect(get).toHaveBeenCalledOnce();
    expect(send.mock.calls.filter(([channel]) => channel === 'app:state-changed')).toHaveLength(1);

    settings = {
      ...settings,
      transcription: { ...settings.transcription, language: 'fr' },
    };
    for (const listener of listeners) listener(settings);
    expect(send.mock.calls.filter(([channel]) => channel === 'settings:changed')).toHaveLength(1);
    expect(send.mock.calls.filter(([channel]) => channel === 'app:state-changed')).toHaveLength(1);

    settings = { ...settings, app: { ...settings.app, enabled: false } };
    for (const listener of listeners) listener(settings);
    expect(send.mock.calls.filter(([channel]) => channel === 'app:state-changed')).toHaveLength(2);
    expect(service.getState()).toMatchObject({ enabled: false, status: 'disabled' });
    expect(get).toHaveBeenCalledOnce();
  });
});
