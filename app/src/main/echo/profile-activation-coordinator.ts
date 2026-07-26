import { randomUUID } from 'node:crypto';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
  GENERAL_PROFILE_ID,
  PROMPT_PROFILE_ID,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../shared/schemas/dictation-profiles';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';
import type { EchoHelperPort } from './echo-session-ports';

export class ProfileActivationCoordinator {
  readonly #settings: SettingsStore;
  readonly #helper: EchoHelperPort;
  readonly #isModelReady: () => boolean;
  #activationTail: Promise<void> = Promise.resolve();
  #syncRequested = false;
  #syncScheduled = false;
  #profileTransaction: Promise<void> = Promise.resolve();
  #disposed = false;

  constructor(options: {
    readonly settings: SettingsStore;
    readonly helper: EchoHelperPort;
    readonly isModelReady: () => boolean;
  }) {
    this.#settings = options.settings;
    this.#helper = options.helper;
    this.#isModelReady = options.isModelReady;
  }

  dispose(): void {
    this.#disposed = true;
  }

  requestSync(): void {
    if (this.#disposed) return;
    this.#syncRequested = true;
    if (this.#syncScheduled) return;
    this.#syncScheduled = true;
    const operation = async () => {
      try {
        while (this.#syncRequested && !this.#disposed) {
          this.#syncRequested = false;
          const settings = this.#settings.get().app;
          await this.#helper.configureActivation(
            settings.enabled && this.#isModelReady(),
            profileBindings(this.#settings.get().dictationProfiles),
          );
        }
      } finally {
        this.#syncScheduled = false;
        if (this.#syncRequested) this.requestSync();
      }
    };
    this.#activationTail = this.#activationTail.then(operation, operation).catch(() => undefined);
  }

  async updateGeneral(patch: PublicSettingsPatch, current: Readonly<Settings>): Promise<Settings> {
    const nextEnabled = patch.app?.enabled ?? current.app.enabled;
    if (this.#helper.readiness.status !== 'ready') return this.#settings.update(patch);
    await this.#helper.configureActivation(
      nextEnabled && this.#isModelReady(),
      profileBindings(current.dictationProfiles),
    );
    try {
      return await this.#settings.update(patch);
    } catch (error: unknown) {
      await this.#helper
        .configureActivation(
          current.app.enabled && this.#isModelReady(),
          profileBindings(current.dictationProfiles),
        )
        .catch(() => undefined);
      throw error;
    }
  }

  createProfile(input: DictationProfileCreate): Promise<Settings> {
    return this.#serializeProfileTransaction(() => {
      const profile = { id: randomUUID(), ...DictationProfileCreateSchema.parse(input) };
      return this.#replaceProfiles([...this.#settings.get().dictationProfiles, profile]);
    });
  }

  updateProfile(id: string, patch: DictationProfilePatch): Promise<Settings> {
    return this.#serializeProfileTransaction(() => {
      const parsed = DictationProfilePatchSchema.parse(patch);
      const current = this.#settings.get().dictationProfiles;
      if (!current.some((profile) => profile.id === id)) {
        throw new Error('Dictation profile not found');
      }
      return this.#replaceProfiles(
        DictationProfileListSchema.parse(
          current.map((profile) => (profile.id === id ? { ...profile, ...parsed } : profile)),
        ),
      );
    });
  }

  deleteProfile(id: string): Promise<Settings> {
    return this.#serializeProfileTransaction(() => {
      if (id === GENERAL_PROFILE_ID || id === PROMPT_PROFILE_ID) {
        throw new Error('Built-in dictation profiles cannot be deleted');
      }
      const current = this.#settings.get().dictationProfiles;
      if (!current.some((profile) => profile.id === id)) {
        throw new Error('Dictation profile not found');
      }
      return this.#replaceProfiles(current.filter((profile) => profile.id !== id));
    });
  }

  resetProfile(id: string): Promise<Settings> {
    return this.#serializeProfileTransaction(() => {
      const replacement =
        id === GENERAL_PROFILE_ID
          ? DEFAULT_GENERAL_PROFILE
          : id === PROMPT_PROFILE_ID
            ? DEFAULT_PROMPT_PROFILE
            : null;
      if (replacement === null) throw new Error('Only built-in profiles can be reset');
      return this.#replaceProfiles(
        this.#settings
          .get()
          .dictationProfiles.map((profile) => (profile.id === id ? replacement : profile)),
      );
    });
  }

  #serializeProfileTransaction<Result>(operation: () => Promise<Result>): Promise<Result> {
    const result = this.#profileTransaction.then(operation, operation);
    this.#profileTransaction = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  async #replaceProfiles(input: readonly DictationProfile[]): Promise<Settings> {
    const profiles = DictationProfileListSchema.parse(input);
    const current = this.#settings.get();
    if (this.#helper.readiness.status !== 'ready') {
      return this.#settings.update({ dictationProfiles: profiles });
    }
    await this.#helper.configureActivation(
      current.app.enabled && this.#isModelReady(),
      profileBindings(profiles),
    );
    try {
      const saved = await this.#settings.update({ dictationProfiles: profiles });
      // The settings subscription schedules an authoritative activation sync. Keep it inside this
      // transaction so a following CRUD operation cannot leave the helper on an older profile set.
      await this.#activationTail;
      return saved;
    } catch (error: unknown) {
      await this.#helper
        .configureActivation(
          current.app.enabled && this.#isModelReady(),
          profileBindings(current.dictationProfiles),
        )
        .catch(() => undefined);
      throw error;
    }
  }
}

function profileBindings(profiles: readonly DictationProfile[]) {
  return profiles.map(({ activationKey: key, shift }) => ({ key, shift }));
}
