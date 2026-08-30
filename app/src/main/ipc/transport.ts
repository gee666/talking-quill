import { ipcMain, type Event as ElectronEvent, type IpcMainInvokeEvent } from 'electron';
import {
  failureResponseSchema,
  invokeRegistry,
  type InvokeChannel,
  type InvokeRequest,
  type WireResponse,
} from '../../shared/ipc/registry';
import type { WindowRole } from '../../shared/constants/app';
import type { WindowRoleRegistry } from '../app/window-role-registry';
import { authorizeIpc } from '../security/ipc-authorization';
import { PublicAppError, toPublicError } from '../security/public-error';
import type { InvokeHandlerMap } from './types';

export interface IpcMainRegistrar {
  handle(
    channel: string,
    listener: (event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>,
  ): void;
  removeHandler(channel: string): void;
}

export interface IpcTransportLifecycle {
  stopAccepting(preservedChannels?: readonly InvokeChannel[]): void;
  drain(excludedChannels?: readonly InvokeChannel[]): Promise<void>;
  pendingChannels(): readonly InvokeChannel[];
  dispose(): void;
}

interface ActiveInvocation {
  readonly channel: InvokeChannel;
  readonly invalidate: () => void;
  readonly dispose: () => void;
}

export function registerIpcTransport(
  roles: WindowRoleRegistry,
  handlers: InvokeHandlerMap,
  registrar: IpcMainRegistrar = ipcMain,
): IpcTransportLifecycle {
  const channels = Object.keys(invokeRegistry) as InvokeChannel[];
  const active = new Map<Promise<unknown>, ActiveInvocation>();
  const acceptingChannels = new Set<InvokeChannel>(channels);
  let disposed = false;
  for (const channel of channels) {
    registerChannel(
      channel,
      roles,
      handlers,
      active,
      () => acceptingChannels.has(channel),
      registrar,
    );
  }
  const stopAccepting = (preservedChannels: readonly InvokeChannel[] = []): void => {
    const preserved = new Set(preservedChannels);
    for (const channel of [...acceptingChannels]) {
      if (preserved.has(channel)) continue;
      registrar.removeHandler(channel);
      acceptingChannels.delete(channel);
    }
    for (const invocation of active.values()) {
      if (!preserved.has(invocation.channel)) invocation.invalidate();
    }
  };
  return {
    stopAccepting,
    async drain(excludedChannels = []) {
      const excluded = new Set(excludedChannels);
      let pending = [...active].filter(([, value]) => !excluded.has(value.channel));
      while (pending.length > 0) {
        await Promise.allSettled(pending.map(([invocation]) => invocation));
        pending = [...active].filter(([, value]) => !excluded.has(value.channel));
      }
    },
    pendingChannels() {
      return Object.freeze([...new Set([...active.values()].map(({ channel }) => channel))]);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      stopAccepting();
    },
  };
}

function registerChannel<Channel extends InvokeChannel>(
  channel: Channel,
  roles: WindowRoleRegistry,
  handlers: Pick<InvokeHandlerMap, Channel>,
  active: Map<Promise<unknown>, ActiveInvocation>,
  isAccepting: () => boolean,
  registrar: IpcMainRegistrar,
): void {
  const contract = invokeRegistry[channel];
  registrar.handle(channel, (event: IpcMainInvokeEvent, input: unknown) => {
    const invalidation = createInvocationInvalidation(event.sender);
    const invocation = invokeChannel(
      channel,
      event,
      input,
      contract.roles,
      roles,
      handlers,
      isAccepting,
      invalidation.context,
    );
    active.set(invocation, {
      channel,
      invalidate: invalidation.invalidate,
      dispose: invalidation.dispose,
    });
    const remove = (): void => {
      active.get(invocation)?.dispose();
      active.delete(invocation);
    };
    void invocation.then(remove, remove);
    return invocation;
  });
}

async function invokeChannel<Channel extends InvokeChannel>(
  channel: Channel,
  event: IpcMainInvokeEvent,
  input: unknown,
  allowedRoles: readonly WindowRole[],
  roles: WindowRoleRegistry,
  handlers: Pick<InvokeHandlerMap, Channel>,
  isAccepting: () => boolean,
  context: ReturnType<typeof createInvocationInvalidation>['context'],
): Promise<WireResponse<Channel>> {
  try {
    if (!isAccepting()) {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'The application is shutting down.',
      });
    }
    authorize(event, allowedRoles, roles);
    const request = invokeRegistry[channel].request.parse(input) as InvokeRequest<Channel>;
    const output = await handlers[channel](request, context);
    const parsedResponse = invokeRegistry[channel].response.safeParse(output);
    if (!parsedResponse.success) throw new Error('IPC handler returned an invalid response');
    if (channel === 'data:reset-all') {
      // Preparation is already durable and cannot be rolled back merely because the renderer
      // disappeared. Delivery is best-effort; the invoke success is an independent fallback.
      try {
        if (!event.sender.isDestroyed()) {
          event.sender.send('data:reset-accepted', parsedResponse.data);
        }
      } catch {
        // A forced relaunch still proceeds after durable reset preparation.
      }
    }
    return { ok: true, data: parsedResponse.data } as WireResponse<Channel>;
  } catch (error: unknown) {
    return failureResponseSchema.parse({ ok: false, error: toPublicError(error) });
  }
}

function authorize(
  event: IpcMainInvokeEvent,
  allowedRoles: readonly WindowRole[],
  roles: WindowRoleRegistry,
): void {
  const registered = roles.get(event.sender.id);
  const frame = event.senderFrame;
  authorizeIpc({
    registeredRole: registered?.role ?? null,
    allowedRoles,
    isMainFrame: !event.sender.isDestroyed() && frame !== null && frame === event.sender.mainFrame,
    frameUrl: frame?.url ?? '',
    expectedUrl: registered?.expectedUrl ?? null,
  });
}

function createInvocationInvalidation(sender: IpcMainInvokeEvent['sender']) {
  const listeners = new Set<() => void>();
  let active = true;
  const invalidate = (): void => {
    if (!active) return;
    active = false;
    removeSenderInvalidation();
    for (const listener of [...listeners]) {
      try {
        listener();
      } catch {
        // Cancellation of one operation must not block the remaining owners.
      }
    }
    listeners.clear();
  };
  const removeSenderInvalidation = subscribeToRendererInvalidation(sender, invalidate);
  return {
    context: {
      webContentsId: sender.id,
      onDestroyed(listener: () => void) {
        if (!active) {
          listener();
          return () => undefined;
        }
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    },
    invalidate,
    dispose: () => {
      if (!active) return;
      active = false;
      removeSenderInvalidation();
      listeners.clear();
    },
  } as const;
}

export function subscribeToRendererInvalidation(
  sender: IpcMainInvokeEvent['sender'],
  listener: () => void,
): () => void {
  let active = true;
  const cleanup = () => {
    if (!active) return;
    active = false;
    sender.off('destroyed', invalidate);
    sender.off('render-process-gone', invalidate);
    sender.off('did-start-navigation', onDidStartNavigation);
    sender.off('did-navigate', invalidate);
  };
  const invalidate = () => {
    if (!active) return;
    cleanup();
    listener();
  };
  const onDidStartNavigation = (
    _event: ElectronEvent,
    _url: string,
    isInPlace: boolean,
    isMainFrame: boolean,
  ) => {
    if (isMainFrame && !isInPlace) invalidate();
  };
  sender.on('destroyed', invalidate);
  sender.on('render-process-gone', invalidate);
  sender.on('did-start-navigation', onDidStartNavigation);
  sender.on('did-navigate', invalidate);
  return cleanup;
}
