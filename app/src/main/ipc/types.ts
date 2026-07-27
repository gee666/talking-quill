import type { InvokeChannel, InvokeRequest, InvokeResponse } from '../../shared/ipc/registry';

export interface AuthorizedIpcContext {
  readonly webContentsId: number;
  /** Fires once when the sender, renderer process, or current main-frame document is replaced. */
  readonly onDestroyed: (listener: () => void) => () => void;
}

export type InvokeHandler<Channel extends InvokeChannel> = (
  request: InvokeRequest<Channel>,
  context: AuthorizedIpcContext,
) => InvokeResponse<Channel> | Promise<InvokeResponse<Channel>>;

export type InvokeHandlerMap = {
  readonly [Channel in InvokeChannel]: InvokeHandler<Channel>;
};
