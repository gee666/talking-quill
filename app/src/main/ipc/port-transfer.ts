import type { MessagePortMain, WebContents } from 'electron';
import {
  portTransferRegistry,
  type PortTransferChannel,
  type PortTransferDescriptor,
  type PortTransferRole,
} from '../../shared/ipc/registry';

export function transferPort<Channel extends PortTransferChannel>(
  target: WebContents,
  registeredRole: string | null,
  role: PortTransferRole<Channel>,
  channel: Channel,
  descriptor: PortTransferDescriptor<Channel>,
  port: MessagePortMain,
): void {
  const contract = portTransferRegistry[channel];
  if (registeredRole !== role || !(contract.roles as readonly string[]).includes(role)) {
    throw new Error(`MessagePort target role is not allowed for ${channel}`);
  }
  const parsed: unknown = contract.descriptor.parse(descriptor);
  target.postMessage(channel, parsed, [port]);
}
