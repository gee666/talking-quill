import { describe, expect, it, vi } from 'vitest';
import { deepFreeze } from '../../app/src/shared/deep-freeze';
import { deepFreeze as freezePreloadApi } from '../../app/src/preload/transport';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import {
  BUILT_IN_DICTATION_PROFILE_METADATA,
  DEFAULT_GENERAL_PROFILE,
  defaultDictationProfiles,
} from '../../app/src/shared/schemas/dictation-profiles';
import { invokeRegistry } from '../../app/src/shared/ipc/registry';
import { applicationInvokes } from '../../app/src/shared/ipc/invoke-application';
import { providerInvokes } from '../../app/src/shared/ipc/invoke-provider';
import { contentInvokes } from '../../app/src/shared/ipc/invoke-content';

vi.mock('electron', () => ({ ipcRenderer: {} }));

describe('shared freeze behavior', () => {
  for (const freeze of [deepFreeze, freezePreloadApi]) {
    it(`${freeze.name} freezes nested objects in place and handles cycles`, () => {
      const value = { children: [{ enabled: true }], self: null as unknown };
      value.self = value;
      expect(freeze(value)).toBe(value);
      expect(Object.isFrozen(value)).toBe(true);
      expect(Object.isFrozen(value.children)).toBe(true);
      expect(Object.isFrozen(value.children[0])).toBe(true);
    });

    it(`${freeze.name} preserves the existing early returns`, () => {
      const nested = { enabled: true };
      const frozen = Object.freeze({ nested });
      expect(freeze(frozen)).toBe(frozen);
      expect(Object.isFrozen(nested)).toBe(false);
      const callback = () => undefined;
      expect(freeze(callback)).toBe(callback);
      expect(Object.isFrozen(callback)).toBe(false);
      for (const primitive of [null, undefined, false, 0, 'text']) {
        expect(freeze(primitive)).toBe(primitive);
      }
    });
  }

  it('keeps defaults frozen and profile clones mutable', () => {
    expect(DEFAULT_GENERAL_PROFILE).toBe(BUILT_IN_DICTATION_PROFILE_METADATA[0].defaultProfile);
    expect(Object.isFrozen(DEFAULT_GENERAL_PROFILE.shortcut.keys)).toBe(true);
    expect(Object.isFrozen(DEFAULT_SETTINGS.smartProcessing.providers.ollama)).toBe(true);
    const profiles = defaultDictationProfiles();
    expect(profiles[0]).toEqual(DEFAULT_GENERAL_PROFILE);
    expect(profiles[0]).not.toBe(DEFAULT_GENERAL_PROFILE);
    expect(Object.isFrozen(profiles[0])).toBe(false);
  });
});

describe('split invoke registry', () => {
  it('preserves definition identity, freezing, and ordered disjoint channel groups', () => {
    const groups = [applicationInvokes, providerInvokes, contentInvokes];
    const channels = groups.flatMap((group) => Object.keys(group));
    expect(Object.keys(invokeRegistry)).toEqual(channels);
    expect(new Set(channels).size).toBe(channels.length);
    expect(Object.isFrozen(invokeRegistry)).toBe(true);
    for (const group of groups) {
      for (const [channel, definition] of Object.entries(group)) {
        expect(Reflect.get(invokeRegistry, channel)).toBe(definition);
        expect(Object.isFrozen(definition)).toBe(true);
      }
    }
    expect(invokeRegistry['welcome:complete'].request).toBe(
      invokeRegistry['provider:catalog'].request,
    );
    expect(invokeRegistry['info:open-location'].response).toBe(
      invokeRegistry['widget:stop'].response,
    );
  });
});
