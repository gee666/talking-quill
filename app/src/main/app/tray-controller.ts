import { Menu, Tray, nativeImage, type NativeImage } from 'electron';
import { APP_NAME } from '../../shared/constants/app';
import type { AppStateService } from './app-state-service';

// Single-colour silhouette so the icon stays crisp at 16px and works as a macOS template image.
const TRAY_SVG = `
<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">
  <path d="M25.4 2.6C18.2 3.6 13.2 9.2 12 21.6c5.9-2.5 11.4-8.9 13.4-19Z" fill="#F6EBD6"/>
  <path d="M12.4 20.4 7.6 29.4" stroke="#F6EBD6" stroke-width="2.2" stroke-linecap="round"/>
</svg>`;

export interface TrayActions {
  readonly showMain: () => void;
  readonly quit: () => void;
  readonly setEnabled: (enabled: boolean) => Promise<void>;
}

export class TrayController {
  readonly #state: AppStateService;
  readonly #actions: TrayActions;
  readonly #tray: Tray;
  #notice: string | null = null;

  constructor(state: AppStateService, actions: TrayActions) {
    this.#state = state;
    this.#actions = actions;
    this.#tray = new Tray(createTrayImage());
    this.#tray.setToolTip(APP_NAME);
    this.#tray.on('click', actions.showMain);
    this.refresh();
  }

  refresh(clearNotice = false): void {
    if (clearNotice) this.#notice = null;
    const enabled = this.#state.getState().enabled;
    this.#tray.setContextMenu(
      Menu.buildFromTemplate([
        { label: `Open ${APP_NAME}`, click: this.#actions.showMain },
        { type: 'separator' },
        {
          label: enabled ? 'Disable' : 'Enable',
          click: () => void this.#setEnabled(!enabled),
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

  destroy(): void {
    this.#tray.destroy();
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

function createTrayImage(): NativeImage {
  const data = Buffer.from(TRAY_SVG).toString('base64');
  const image = nativeImage.createFromDataURL(`data:image/svg+xml;base64,${data}`);
  image.setTemplateImage(process.platform === 'darwin');
  const size = process.platform === 'darwin' ? 18 : 16;
  return image.resize({ width: size, height: size });
}
