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
      await mapOpenFailure(this.#openMicrophoneSettings);
      return;
    }
    if (process.platform !== 'darwin') {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'This permission pane is unavailable.',
      });
    }
    await mapOpenFailure(() => shell.openExternal(MAC_PERMISSION_URLS[permission]));
  }

  async openLocation(location: InfoLocation): Promise<void> {
    await mapOpenFailure(async () => {
      const error = await shell.openPath(location === 'data' ? this.#paths.root : this.#paths.logs);
      if (error.length > 0) throw new Error('Electron could not open the folder');
    });
  }

  async openRelease(value: string): Promise<void> {
    const releaseUrl = validateReleaseUrl(value);
    await mapOpenFailure(() => shell.openExternal(releaseUrl));
  }
}

async function mapOpenFailure(operation: () => Promise<unknown>): Promise<void> {
  try {
    await operation();
  } catch {
    throw new PublicAppError({
      code: 'UNAVAILABLE',
      message: 'The requested system location could not be opened.',
    });
  }
}
