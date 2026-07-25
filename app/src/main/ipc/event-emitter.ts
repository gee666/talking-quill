import type { WebContents } from 'electron';
import { eventRegistry, type EventChannel, type EventPayload } from '../../shared/ipc/registry';
import type { WindowRoleRegistry } from '../app/window-role-registry';

export class IpcEventEmitter {
  readonly #roles: WindowRoleRegistry;
  readonly #targets: () => readonly WebContents[];

  constructor(roles: WindowRoleRegistry, targets: () => readonly WebContents[]) {
    this.#roles = roles;
    this.#targets = targets;
  }

  send<Channel extends EventChannel>(channel: Channel, payload: EventPayload<Channel>): void {
    const descriptor = eventRegistry[channel];
    const validated = descriptor.payload.parse(payload);
    for (const target of this.#targets()) {
      const registration = this.#roles.get(target.id);
      if (
        !target.isDestroyed() &&
        registration !== null &&
        descriptor.roles.some((allowedRole) => allowedRole === registration.role)
      ) {
        try {
          target.send(channel, validated);
        } catch {
          // A renderer can disappear between the destruction check and event delivery.
        }
      }
    }
  }
}
