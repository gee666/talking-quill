import type { InvokeChannel, InvokeRequest, InvokeResponse } from '../../shared/ipc/registry';
import type { WindowRole } from '../../shared/constants/app';

export interface AuthorizedIpcContext {
  readonly role: WindowRole;
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
