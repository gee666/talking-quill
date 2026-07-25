import { contextBridge } from 'electron';
import type { CaptureApi } from '../shared/bridge/api';
import { deepFreeze, forwardTransferredPort, invoke } from './transport';

forwardTransferredPort('capture:port');

const api: CaptureApi = {
  ready: async () => {
    await invoke('capture:ready', {});
  },
};

contextBridge.exposeInMainWorld('talkingQuillCapture', deepFreeze(api));
