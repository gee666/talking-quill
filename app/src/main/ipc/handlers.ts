import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';
import { createApplicationHandlers } from './application-handlers';
import { createSettingsHandlers } from './settings-handlers';
import { createProviderHandlers } from './provider-handlers';
import { createRendererHandlers } from './renderer-handlers';
import { createContentHandlers } from './content-handlers';

export type { HandlerDependencies } from './handler-dependencies';

export function createHandlers(dependencies: HandlerDependencies): InvokeHandlerMap {
  // Transport registration order comes from invokeRegistry, not this object's insertion order.
  return {
    ...createApplicationHandlers(dependencies),
    ...createSettingsHandlers(dependencies),
    ...createProviderHandlers(dependencies),
    ...createRendererHandlers(dependencies),
    ...createContentHandlers(dependencies),
  };
}
