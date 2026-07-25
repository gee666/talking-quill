import type { WebContents } from 'electron';
import type { WindowRole } from '../../shared/constants/app';

interface RegisteredWindow {
  readonly role: WindowRole;
  expectedUrl: string;
}

export class WindowRoleRegistry {
  readonly #windows = new Map<number, RegisteredWindow>();

  register(webContents: WebContents, role: WindowRole, expectedUrl: string): void {
    this.#windows.set(webContents.id, { role, expectedUrl });
    webContents.once('destroyed', () => this.#windows.delete(webContents.id));
  }

  get(webContentsId: number): Readonly<RegisteredWindow> | null {
    return this.#windows.get(webContentsId) ?? null;
  }

  unregister(webContentsId: number): void {
    this.#windows.delete(webContentsId);
  }
}
