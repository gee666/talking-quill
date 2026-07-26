import type { InvokeChannel, InvokeRequest, InvokeResponse } from '../../shared/ipc/registry';

export interface AuthorizedIpcContext {
  readonly webContentsId: number;
  readonly onDestroyed: (listener: () => void) => () => void;
}

export type InvokeHandler<Channel extends InvokeChannel> = (
  request: InvokeRequest<Channel>,
  context: AuthorizedIpcContext,
) => InvokeResponse<Channel> | Promise<InvokeResponse<Channel>>;

export type InvokeHandlerMap = {
  readonly [Channel in InvokeChannel]: InvokeHandler<Channel>;
};
