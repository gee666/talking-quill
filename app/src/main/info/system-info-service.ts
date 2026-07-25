import { shell } from 'electron';
import type { InfoLocation, InfoPermission } from '../../shared/schemas/info';
import type { AppPaths } from '../persistence/paths';
import { PublicAppError } from '../security/public-error';
import { validateReleaseUrl } from './release-url-policy';

const MAC_PERMISSION_URLS: Readonly<Record<Exclude<InfoPermission, 'microphone'>, string>> = {
  accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
  'input-monitoring': 'x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent',
  'screen-recording':
    'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
};

export class SystemInfoService {
  readonly #paths: AppPaths;
  readonly #openMicrophoneSettings: () => Promise<void>;

  constructor(paths: AppPaths, openMicrophoneSettings: () => Promise<void>) {
    this.#paths = paths;
    this.#openMicrophoneSettings = openMicrophoneSettings;
  }

  async openPermission(permission: InfoPermission): Promise<void> {
    if (permission === 'microphone') {
      await this.#openMicrophoneSettings();
      return;
    }
    if (process.platform !== 'darwin') {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'This permission pane is unavailable.',
      });
    }
    await shell.openExternal(MAC_PERMISSION_URLS[permission]);
  }

  async openLocation(location: InfoLocation): Promise<void> {
    const error = await shell.openPath(location === 'data' ? this.#paths.root : this.#paths.logs);
    if (error.length > 0) {
      throw new PublicAppError({ code: 'UNAVAILABLE', message: 'The folder could not be opened.' });
    }
  }

  async openRelease(value: string): Promise<void> {
    await shell.openExternal(validateReleaseUrl(value));
  }
}
