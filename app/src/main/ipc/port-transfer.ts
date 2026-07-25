import type { MessagePortMain, WebContents } from 'electron';
import {
  portTransferRegistry,
  type PortTransferChannel,
  type PortTransferDescriptor,
} from '../../shared/ipc/registry';

export function transferPort<Channel extends PortTransferChannel>(
  target: WebContents,
  channel: Channel,
  descriptor: PortTransferDescriptor<Channel>,
  port: MessagePortMain,
): void {
  const parsed: unknown = portTransferRegistry[channel].descriptor.parse(descriptor);
  target.postMessage(channel, parsed, [port]);
}
