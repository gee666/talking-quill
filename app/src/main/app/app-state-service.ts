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
  #enabled: boolean;
  #state: AppState;

  constructor(settings: SettingsStore, events: IpcEventEmitter) {
    this.#settings = settings;
    this.#events = events;
    this.#enabled = settings.get().app.enabled;
    this.#state = this.#createState();
    this.#settings.subscribe((next) => {
      this.#events.send('settings:changed', next);
      if (this.#enabled === next.app.enabled) return;
      this.#enabled = next.app.enabled;
      this.#publishStateChange();
    });
  }

  getState(): AppState {
    return structuredClone(this.#state);
  }

  getSettings(): Settings {
    return this.#settings.get();
  }

  setSession(snapshot: EchoSessionSnapshot): void {
    if (this.#sessionPhase === snapshot.phase) return;
    this.#sessionPhase = snapshot.phase;
    this.#publishStateChange();
  }

  get modelReady(): boolean {
    return this.#modelReady;
  }

  setModelReady(ready: boolean): void {
    if (this.#modelReady === ready) return;
    this.#modelReady = ready;
    this.#publishStateChange();
  }

  setHelperReadiness(readiness: HelperReadiness): void {
    if (helperReadinessEquals(this.#helperReadiness, readiness)) return;
    this.#helperReadiness = readiness;
    this.#publishStateChange();
  }

  async updateSettings(patch: PublicSettingsPatch): Promise<Settings> {
    return this.#settings.update(PublicSettingsPatchSchema.parse(patch));
  }

  #createState(): AppState {
    return AppStateSchema.parse({
      enabled: this.#enabled,
      status: this.#status(),
      modelReady: this.#modelReady,
      helper: this.#helperReadiness,
    });
  }

  #publishStateChange(): void {
    const next = this.#createState();
    if (appStateEquals(this.#state, next)) return;
    this.#state = next;
    this.#events.send('app:state-changed', next);
  }

  #status(): AppState['status'] {
    if (!this.#enabled) return 'disabled';
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

function appStateEquals(first: AppState, second: AppState): boolean {
  return (
    first.enabled === second.enabled &&
    first.status === second.status &&
    first.modelReady === second.modelReady &&
    helperReadinessEquals(first.helper, second.helper)
  );
}

function helperReadinessEquals(first: HelperReadiness, second: HelperReadiness): boolean {
  return (
    first.status === second.status &&
    first.reason === second.reason &&
    first.helperVersion === second.helperVersion &&
    first.permissions.accessibility === second.permissions.accessibility &&
    first.permissions.inputMonitoring === second.permissions.inputMonitoring &&
    first.permissions.eventPost === second.permissions.eventPost
  );
}
