import { ipcMain, type IpcMainInvokeEvent } from 'electron';
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
  dispose(): void;
}

export function registerIpcTransport(
  roles: WindowRoleRegistry,
  handlers: InvokeHandlerMap,
  registrar: IpcMainRegistrar = ipcMain,
): IpcTransportLifecycle {
  const channels = Object.keys(invokeRegistry) as InvokeChannel[];
  const active = new Map<Promise<unknown>, InvokeChannel>();
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
  };
  return {
    stopAccepting,
    async drain(excludedChannels = []) {
      const excluded = new Set(excludedChannels);
      let pending = [...active].filter(([, channel]) => !excluded.has(channel));
      while (pending.length > 0) {
        await Promise.allSettled(pending.map(([invocation]) => invocation));
        pending = [...active].filter(([, channel]) => !excluded.has(channel));
      }
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
  active: Map<Promise<unknown>, InvokeChannel>,
  isAccepting: () => boolean,
  registrar: IpcMainRegistrar,
): void {
  const contract = invokeRegistry[channel];
  registrar.handle(channel, (event: IpcMainInvokeEvent, input: unknown) => {
    const invocation = invokeChannel(
      channel,
      event,
      input,
      contract.roles,
      roles,
      handlers,
      isAccepting,
    );
    active.set(invocation, channel);
    const remove = (): void => {
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
): Promise<WireResponse<Channel>> {
  try {
    if (!isAccepting()) {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'The application is shutting down.',
      });
    }
    const context = authorize(event, allowedRoles, roles);
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
) {
  const registered = roles.get(event.sender.id);
  const frame = event.senderFrame;
  const role = authorizeIpc({
    registeredRole: registered?.role ?? null,
    allowedRoles,
    isMainFrame: !event.sender.isDestroyed() && frame !== null && frame === event.sender.mainFrame,
    frameUrl: frame?.url ?? '',
    expectedUrl: registered?.expectedUrl ?? null,
  });
  return {
    role,
    webContentsId: event.sender.id,
    onDestroyed: (listener: () => void) => {
      event.sender.once('destroyed', listener);
      return () => event.sender.off('destroyed', listener);
    },
  } as const;
}
