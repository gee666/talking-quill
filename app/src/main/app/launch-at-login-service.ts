import { PublicAppError } from '../security/public-error';

export interface LoginItemAdapter {
  getLoginItemSettings(): { readonly openAtLogin: boolean };
  setLoginItemSettings(settings: { readonly openAtLogin: boolean }): void;
}

export function clearLaunchAtLoginForUninstall(adapter: LoginItemAdapter): void {
  adapter.setLoginItemSettings({ openAtLogin: false });
  if (adapter.getLoginItemSettings().openAtLogin) throw registrationFailure();
}

/** Truthfully reconciles requested and OS-observed login registration. */
export class LaunchAtLoginService {
  readonly #adapter: LoginItemAdapter;
  #disposed = false;

  constructor(adapter: LoginItemAdapter) {
    this.#adapter = adapter;
  }

  reconcile(requested: boolean): boolean {
    this.#assertActive();
    const observed = this.#adapter.getLoginItemSettings().openAtLogin;
    if (observed !== requested) this.#adapter.setLoginItemSettings({ openAtLogin: requested });
    const confirmed = this.#adapter.getLoginItemSettings().openAtLogin;
    if (confirmed !== requested) throw registrationFailure();
    return confirmed;
  }

  set(enabled: boolean): void {
    this.#assertActive();
    try {
      this.#adapter.setLoginItemSettings({ openAtLogin: enabled });
      if (this.#adapter.getLoginItemSettings().openAtLogin !== enabled) throw registrationFailure();
    } catch (error: unknown) {
      if (error instanceof PublicAppError) throw error;
      throw registrationFailure();
    }
  }

  dispose(): void {
    this.#disposed = true;
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
