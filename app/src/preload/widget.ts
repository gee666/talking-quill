import { contextBridge } from 'electron';
import type { WidgetApi } from '../shared/bridge/api';
import { deepFreeze, invoke, subscribe } from './transport';

const api: WidgetApi = {
  ready: () => invoke('widget:ready', {}),
  stop: async () => {
    await invoke('widget:stop', {});
  },
  cancel: async () => {
    await invoke('widget:cancel', {});
  },
  setInteractive: async (interactive) => {
    await invoke('widget:set-interactive', { interactive });
  },
  onSessionChanged: (listener) => subscribe('echo:session-changed', listener),
};

contextBridge.exposeInMainWorld('talkingQuillWidget', deepFreeze(api));
