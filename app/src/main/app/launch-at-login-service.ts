import { PublicAppError } from '../security/public-error';

export const WINDOWS_LOGIN_START_ARGUMENT = '--talking-quill-login-start';

export type WindowsLoginStartClassification = 'absent' | 'login-start' | 'invalid';

export function classifyWindowsLoginStartArguments(
  arguments_: readonly string[],
  packaged: boolean,
  platform: NodeJS.Platform,
): WindowsLoginStartClassification {
  const markerLike = arguments_.filter((argument) =>
    argument.startsWith(WINDOWS_LOGIN_START_ARGUMENT),
  );
  if (markerLike.length === 0) return 'absent';
  if (
    !packaged ||
    platform !== 'win32' ||
    markerLike.length !== 1 ||
    markerLike[0] !== WINDOWS_LOGIN_START_ARGUMENT ||
    arguments_.includes('--talking-quill-request-machine-quit')
  ) {
    return 'invalid';
  }
  return 'login-start';
}

export interface LoginItemAdapter {
  getLoginItemSettings(settings?: { readonly args?: string[] }): { readonly openAtLogin: boolean };
  setLoginItemSettings(settings: { readonly openAtLogin: boolean; readonly args?: string[] }): void;
}

export function clearLaunchAtLoginForUninstall(adapter: LoginItemAdapter): void {
  adapter.setLoginItemSettings({ openAtLogin: false });
  if (adapter.getLoginItemSettings().openAtLogin) throw registrationFailure();
}

/** Truthfully reconciles requested and OS-observed login registration. */
export class LaunchAtLoginService {
  readonly #adapter: LoginItemAdapter;
  readonly #platform: NodeJS.Platform;
  #disposed = false;

  constructor(adapter: LoginItemAdapter, platform: NodeJS.Platform = process.platform) {
    this.#adapter = adapter;
    this.#platform = platform;
  }

  reconcile(requested: boolean): boolean {
    this.#assertActive();
    const observed = this.#getLoginItemSettings().openAtLogin;
    if (observed !== requested || (this.#platform === 'win32' && requested)) {
      this.#setLoginItem(requested);
    }
    const confirmed = this.#getLoginItemSettings().openAtLogin;
    if (confirmed !== requested) throw registrationFailure();
    return confirmed;
  }

  set(enabled: boolean): void {
    this.#assertActive();
    try {
      this.#setLoginItem(enabled);
      if (this.#getLoginItemSettings().openAtLogin !== enabled) throw registrationFailure();
    } catch (error: unknown) {
      if (error instanceof PublicAppError) throw error;
      throw registrationFailure();
    }
  }

  dispose(): void {
    this.#disposed = true;
  }

  #getLoginItemSettings(): { readonly openAtLogin: boolean } {
    return this.#adapter.getLoginItemSettings(
      this.#platform === 'win32' ? { args: [WINDOWS_LOGIN_START_ARGUMENT] } : undefined,
    );
  }

  #setLoginItem(openAtLogin: boolean): void {
    this.#adapter.setLoginItemSettings({
      openAtLogin,
      ...(this.#platform === 'win32' && openAtLogin
        ? { args: [WINDOWS_LOGIN_START_ARGUMENT] }
        : {}),
    });
  }

  #assertActive(): void {
    if (this.#disposed) {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'Startup registration is unavailable.',
      });
    }
  }
}

function registrationFailure(): PublicAppError {
  return new PublicAppError({
    code: 'UNAVAILABLE',
    message: 'Launch at login could not be registered with the operating system.',
  });
}
