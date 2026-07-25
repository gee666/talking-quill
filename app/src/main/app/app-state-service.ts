import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import {
  PublicSettingsPatchSchema,
  type PublicSettingsPatch,
  type Settings,
} from '../../shared/schemas/settings';
import { AppStateSchema, type AppState } from '../../shared/schemas/app-state';
import {
  INITIAL_HELPER_READINESS,
  type HelperReadiness,
} from '../../shared/schemas/helper-readiness';
import type { EchoSessionSnapshot } from '../../shared/schemas/echo-session';

export class AppStateService {
  readonly #settings: SettingsStore;
  readonly #events: IpcEventEmitter;
  #helperReadiness: HelperReadiness = INITIAL_HELPER_READINESS;
  #sessionPhase: EchoSessionSnapshot['phase'] = 'idle';
  #modelReady = false;

  constructor(settings: SettingsStore, events: IpcEventEmitter) {
    this.#settings = settings;
    this.#events = events;
    this.#settings.subscribe((next) => {
      this.#events.send('settings:changed', next);
      this.#events.send('app:state-changed', this.getState());
    });
  }

  getState(): AppState {
    return AppStateSchema.parse({
      enabled: this.#settings.get().app.enabled,
      status: this.#status(),
      modelReady: this.#modelReady,
      helper: this.#helperReadiness,
    });
  }

  getSettings(): Settings {
    return this.#settings.get();
  }

  setSession(snapshot: EchoSessionSnapshot): void {
    this.#sessionPhase = snapshot.phase;
    this.#events.send('app:state-changed', this.getState());
  }

  get modelReady(): boolean {
    return this.#modelReady;
  }

  setModelReady(ready: boolean): void {
    this.#modelReady = ready;
    this.#events.send('app:state-changed', this.getState());
  }

  setHelperReadiness(readiness: HelperReadiness): void {
    this.#helperReadiness = readiness;
    this.#events.send('app:state-changed', this.getState());
  }

  async setEnabled(enabled: boolean): Promise<AppState> {
    await this.updateSettings({ app: { enabled } });
    return this.getState();
  }

  async updateSettings(patch: PublicSettingsPatch): Promise<Settings> {
    return this.#settings.update(PublicSettingsPatchSchema.parse(patch));
  }

  #status(): AppState['status'] {
    if (!this.#settings.get().app.enabled) return 'disabled';
    if (this.#sessionPhase === 'arming' || this.#sessionPhase.startsWith('recording')) {
      return 'recording';
    }
    if (this.#sessionPhase === 'transcribing') return 'transcribing';
    if (this.#sessionPhase === 'processingSmart' || this.#sessionPhase === 'inserting') {
      return 'processing';
    }
    return this.#helperReadiness.status === 'ready' && this.#modelReady ? 'ready' : 'needs-setup';
  }
}
