import { Menu, Tray } from 'electron';
import { APP_NAME } from '../../shared/constants/app';
import { createTrayImage } from './tray-icon';
import type { AppStateService } from './app-state-service';

export interface TrayActions {
  readonly showMain: () => void;
  readonly quit: () => void;
  readonly setEnabled: (enabled: boolean) => Promise<void>;
}

export class TrayController {
  readonly #state: AppStateService;
  readonly #actions: TrayActions;
  readonly #tray: Tray;
  readonly #pendingMutations = new Set<Promise<void>>();
  #notice: string | null = null;
  #accepting = true;
  #destroyed = false;

  constructor(state: AppStateService, actions: TrayActions) {
    this.#state = state;
    this.#actions = actions;
    this.#tray = new Tray(createTrayImage());
    this.#tray.setToolTip(APP_NAME);
    this.#tray.on('click', actions.showMain);
    this.refresh();
  }

  refresh(clearNotice = false): void {
    if (this.#destroyed) return;
    if (clearNotice) this.#notice = null;
    const enabled = this.#state.getState().enabled;
    this.#tray.setContextMenu(
      Menu.buildFromTemplate([
        { label: `Open ${APP_NAME}`, click: this.#actions.showMain },
        { type: 'separator' },
        {
          label: enabled ? 'Disable' : 'Enable',
          enabled: this.#accepting,
          click: () => this.#startSetEnabled(!enabled),
        },
        {
          label: this.#notice ?? '',
          enabled: false,
          visible: this.#notice !== null,
        },
        { type: 'separator' },
        { label: 'Quit', click: this.#actions.quit },
      ]),
    );
  }

  stopAccepting(): void {
    if (!this.#accepting) return;
    this.#accepting = false;
    this.refresh();
  }

  async drain(): Promise<void> {
    while (this.#pendingMutations.size > 0) {
      await Promise.allSettled([...this.#pendingMutations]);
    }
  }

  destroy(): void {
    if (this.#destroyed) return;
    this.#accepting = false;
    this.#destroyed = true;
    this.#tray.destroy();
  }

  #startSetEnabled(enabled: boolean): void {
    if (!this.#accepting || this.#destroyed) return;
    const mutation = this.#setEnabled(enabled);
    this.#pendingMutations.add(mutation);
    void mutation.finally(() => this.#pendingMutations.delete(mutation));
  }

  async #setEnabled(enabled: boolean): Promise<void> {
    this.#notice = null;
    try {
      await this.#actions.setEnabled(enabled);
    } catch {
      this.#notice = 'Enabled setting was not saved';
    }
    this.refresh();
  }
}
