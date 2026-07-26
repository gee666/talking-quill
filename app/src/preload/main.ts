import { contextBridge } from 'electron';
import type { MainApi } from '../shared/bridge/api';
import { deepFreeze, invoke, subscribe } from './transport';

const api: MainApi = {
  welcome: {
    setStep: (step) => invoke('welcome:set-step', { step }),
    complete: () => invoke('welcome:complete', {}),
  },
  info: {
    status: () => invoke('info:status', {}),
    checkForUpdates: (operationId) => invoke('info:check-update', { operationId }),
    cancel: async (operationId) => (await invoke('info:cancel-update', { operationId })).cancelled,
    openPermissionSettings: async (permission) => {
      await invoke('info:open-permission', { permission });
    },
    openLocation: async (location) => {
      await invoke('info:open-location', { location });
    },
    openRelease: async (url) => {
      await invoke('info:open-release', { url });
    },
    notices: async () => (await invoke('info:notices', {})).text,
  },
  activationTest: {
    start: () => invoke('activation-test:start', {}),
    stop: () => invoke('activation-test:stop', {}),
    onChanged: (listener) => subscribe('activation-test:changed', listener),
  },
  app: {
    getBootstrap: () => invoke('bootstrap:get', {}),
    setEnabled: (enabled) => invoke('app:set-enabled', { enabled }),
    onStateChanged: (listener) => subscribe('app:state-changed', listener),
  },
  settings: {
    update: (patch) => invoke('settings:update', patch),
    onChanged: (listener) => subscribe('settings:changed', listener),
  },
  profiles: {
    create: (input) => invoke('profile:create', input),
    update: (id, patch) => invoke('profile:update', { id, patch }),
    delete: (id) => invoke('profile:delete', { id }),
    reset: (id) => invoke('profile:reset', { id }),
  },
  data: {
    resetAll: async (confirmation) => {
      await invoke('data:reset-all', { confirmation });
    },
    onResetAccepted: (listener) =>
      subscribe('data:reset-accepted', ({ acknowledgementToken }) => {
        listener();
        // Give the owner renderer one paint opportunity before its one-time acknowledgement.
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            void invoke('data:reset-renderer-ack', { acknowledgementToken }).catch(() => undefined);
          }),
        );
      }),
  },
  recording: {
    getDevices: () => invoke('recording:get-devices', {}),
    startTest: () => invoke('recording:start-test', {}),
    stopTest: () => invoke('recording:stop-test', {}),
    openMicrophoneSettings: async () => {
      await invoke('recording:open-microphone-settings', {});
    },
    onDevicesChanged: (listener) => subscribe('recording:devices-changed', listener),
    onTestLevel: (listener) => subscribe('recording:test-level', listener),
    onTestStateChanged: (listener) => subscribe('recording:test-state-changed', listener),
  },
  echo: {
    onSessionChanged: (listener) => subscribe('echo:session-changed', listener),
  },
  history: {
    list: (limit, cursor) =>
      invoke('history:list', {
        ...(limit === undefined ? {} : { limit }),
        ...(cursor === undefined ? {} : { cursor }),
      }),
    delete: (id) => invoke('history:delete', { id }),
    deleteAll: () => invoke('history:delete-all', {}),
    copy: async (id) => {
      await invoke('history:copy', { id });
    },
    thumbnail: async (id) => (await invoke('history:thumbnail', { id }))?.base64 ?? null,
    onChanged: (listener) => subscribe('history:changed', ({ revision }) => listener(revision)),
  },
  commands: {
    list: () => invoke('commands:list', {}),
    create: (input) => invoke('commands:create', input),
    update: (id, patch) => invoke('commands:update', { id, patch }),
    delete: async (id) => (await invoke('commands:delete', { id })).deleted,
    preview: (transcript) => invoke('commands:preview', { transcript }),
  },
  vocabulary: {
    list: () => invoke('vocabulary:list', {}),
    create: (value) => invoke('vocabulary:create', { value }),
    update: (id, value) => invoke('vocabulary:update', { id, value }),
    delete: async (id) => (await invoke('vocabulary:delete', { id })).deleted,
    importFile: () => invoke('vocabulary:import-file', {}),
    exportFile: () => invoke('vocabulary:export-file', {}),
  },
  providers: {
    catalog: async () => (await invoke('provider:catalog', {})).providers,
    piInstallationStatus: () => invoke('provider:pi-installation-status', {}),
    savePiInstallation: (path) => invoke('provider:pi-installation-save', { path }),
    browsePiInstallation: async () => (await invoke('provider:pi-installation-browse', {})).path,
    saveConfig: (config) => invoke('provider:config-save', { config }),
    setSecret: (providerId, expectedBindingToken, secret) =>
      invoke('provider:secret-set', { providerId, expectedBindingToken, secret }),
    secretStatus: (providerId) => invoke('provider:secret-status', { providerId }),
    deleteSecret: (providerId, expectedBindingToken) =>
      invoke('provider:secret-delete', { providerId, expectedBindingToken }),
    listModels: async (providerId, operationId, refresh = false) =>
      (await invoke('provider:list-models', { providerId, operationId, refresh })).models,
    testConnection: (providerId, operationId) =>
      invoke('provider:test-connection', { providerId, operationId }),
    destination: async (providerId, operationId) =>
      (await invoke('provider:destination', { providerId, operationId })).destination,
    cancel: async (operationId) => (await invoke('provider:cancel', { operationId })).cancelled,
    osaStatus: () => invoke('provider:osa-status', {}),
    setOnScreenAwareness: (enabled) => invoke('provider:osa-set', { enabled }),
    verifyVision: (operationId, nonce) => invoke('provider:vision-test', { operationId, nonce }),
    confirmVision: (operationId, verificationId) =>
      invoke('provider:vision-confirm', { operationId, verificationId }),
  },
  models: {
    list: () => invoke('model:list', {}),
    status: (modelId, verify) =>
      invoke('model:status', { modelId, ...(verify === undefined ? {} : { verify }) }),
    download: (modelId) => invoke('model:download', { modelId }),
    pause: (modelId) => invoke('model:pause', { modelId }),
    cancel: (modelId) => invoke('model:cancel', { modelId }),
    retry: (modelId) => invoke('model:retry', { modelId }),
    delete: (modelId) => invoke('model:delete', { modelId }),
    onProgress: (listener) => subscribe('model:progress', listener),
  },
  windowControls: {
    minimize: async () => {
      await invoke('window:minimize', {});
    },
    toggleMaximize: async () => (await invoke('window:toggle-maximize', {})).maximized,
    close: async () => {
      await invoke('window:close', {});
    },
    onMaximizedChanged: (listener) =>
      subscribe('window:maximized-changed', ({ maximized }) => listener(maximized)),
  },
};

contextBridge.exposeInMainWorld('talkingQuill', deepFreeze(api));
