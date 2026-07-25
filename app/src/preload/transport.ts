import { ipcRenderer } from 'electron';
import {
  eventRegistry,
  failureResponseSchema,
  invokeRegistry,
  portTransferRegistry,
  successResponseSchema,
  type EventChannel,
  type EventPayload,
  type InvokeChannel,
  type InvokeRequest,
  type InvokeResponse,
  type PortTransferChannel,
} from '../shared/ipc/registry';
import { CAPTURE_PORT_WINDOW_MESSAGE } from '../shared/constants/audio';

export class RendererApiError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = 'RendererApiError';
    this.code = code;
  }
}

export async function invoke<Channel extends InvokeChannel>(
  channel: Channel,
  input: InvokeRequest<Channel>,
): Promise<InvokeResponse<Channel>> {
  const request = invokeRegistry[channel].request.parse(input);
  const raw: unknown = await ipcRenderer.invoke(channel, request);
  if (typeof raw === 'object' && raw !== null && Reflect.get(raw, 'ok') === false) {
    const failure = failureResponseSchema.parse(raw);
    throw new RendererApiError(failure.error.code, failure.error.message);
  }
  return successResponseSchema(channel).parse(raw).data;
}

export function subscribe<Channel extends EventChannel>(
  channel: Channel,
  listener: (payload: EventPayload<Channel>) => void,
): () => void {
  const wrapped = (_event: Electron.IpcRendererEvent, input: unknown) => {
    const parsed = eventRegistry[channel].payload.safeParse(input);
    if (parsed.success) listener(parsed.data as EventPayload<Channel>);
  };
  ipcRenderer.on(channel, wrapped);
  return () => ipcRenderer.removeListener(channel, wrapped);
}

export function forwardTransferredPort(channel: PortTransferChannel): () => void {
  const wrapped = (event: Electron.IpcRendererEvent, input: unknown) => {
    const descriptor = portTransferRegistry[channel].descriptor.safeParse(input);
    if (!descriptor.success || event.ports.length !== 1) return;
    window.postMessage(
      { type: CAPTURE_PORT_WINDOW_MESSAGE, descriptor: descriptor.data },
      window.location.origin,
      event.ports,
    );
  };
  ipcRenderer.on(channel, wrapped);
  return () => ipcRenderer.removeListener(channel, wrapped);
}

export function deepFreeze<Value>(value: Value): Readonly<Value> {
  if (typeof value !== 'object' || value === null || Object.isFrozen(value)) return value;
  Object.freeze(value);
  for (const child of Object.values(value)) deepFreeze(child);
  return value;
}
