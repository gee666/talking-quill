import type { InvokeHandlerMap } from '../../app/src/main/ipc/types';
import type { InvokeRequest, PortTransferDescriptor } from '../../app/src/shared/ipc/registry';

export const validEnabledRequest: InvokeRequest<'app:set-enabled'> = { enabled: true };
export const validCapturePort: PortTransferDescriptor<'capture:port'> = {
  protocolVersion: 2,
};

export const invalidCapturePort: PortTransferDescriptor<'capture:port'> = {
  // @ts-expect-error port descriptors are channel-specific and versioned
  protocolVersion: 1,
};
export const validProviderRequest: InvokeRequest<'provider:list-models'> = {
  providerId: 'ollama',
  operationId: 'models-operation-1',
  refresh: true,
};
export const validProviderConfig: InvokeRequest<'provider:config-save'> = {
  config: { providerId: 'ollama', baseUrl: 'http://127.0.0.1:11434' },
};
export const nativeProviderConfig: InvokeRequest<'provider:config-save'> = {
  config: { providerId: 'anthropic', modelId: 'claude-sonnet-4-6' },
};

export const nativeProviderRequest: InvokeRequest<'provider:list-models'> = {
  providerId: 'anthropic',
  operationId: 'models-operation-2',
  refresh: false,
};

export const validProfileBindingUpdate: InvokeRequest<'profile:update'> = {
  id: 'general',
  patch: {
    shortcut: {
      modifiers: { ctrl: true, alt: false, shift: true, meta: false },
      keys: ['P'],
    },
    name: 'Moved General',
  },
};
export const validProfileNonBindingUpdate: InvokeRequest<'profile:update'> = {
  id: 'general',
  patch: { processingMode: 'smart' },
};
export const invalidProfileKeysOnlyUpdate: InvokeRequest<'profile:update'> = {
  id: 'general',
  // @ts-expect-error shortcut updates require the complete canonical shape
  patch: { shortcut: { keys: ['Q'] } },
};
export const invalidProfileModifiersOnlyUpdate: InvokeRequest<'profile:update'> = {
  id: 'general',
  patch: {
    // @ts-expect-error shortcut updates require ordered keys
    shortcut: { modifiers: { ctrl: false, alt: true, shift: true, meta: false } },
  },
};

// @ts-expect-error adding an unregistered channel is a compile-time failure
export type UnknownChannelRequest = InvokeRequest<'ipc:unknown'>;

// @ts-expect-error every registered channel must have a handler
export const incompleteHandlers: InvokeHandlerMap = {
  'bootstrap:get': () => {
    throw new Error('compile-only fixture');
  },
};
