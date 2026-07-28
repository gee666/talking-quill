import { randomUUID } from 'node:crypto';
import {
  BuiltInDictationProfileIdSchema,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
  builtInDictationProfile,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../shared/schemas/dictation-profiles';
import type { ActivationBinding } from '../../shared/helper/protocol';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';
import type { EchoHelperPort } from './echo-session-ports';

export class ProfileActivationCoordinator {
  readonly #settings: SettingsStore;
  readonly #helper: EchoHelperPort;
  readonly #isModelReady: () => boolean;
  readonly #onSyncFailure: (error: unknown) => void;
  #transactionTail: Promise<void> = Promise.resolve();
  #transactionActive = false;
  #syncRequested = false;
  #syncScheduled = false;
  readonly #shortcutCaptureOwners = new Set<number>();
  #disposed = false;

  constructor(options: {
    readonly settings: SettingsStore;
    readonly helper: EchoHelperPort;
    readonly isModelReady: () => boolean;
    readonly onSyncFailure?: (error: unknown) => void;
  }) {
    this.#settings = options.settings;
    this.#helper = options.helper;
    this.#isModelReady = options.isModelReady;
    this.#onSyncFailure = options.onSyncFailure ?? (() => undefined);
  }

  dispose(): void {
    this.#disposed = true;
    this.#shortcutCaptureOwners.clear();
  }

  get shortcutCaptureActive(): boolean {
    return this.#shortcutCaptureOwners.size > 0;
  }

  async beginShortcutCapture(ownerWebContentsId: number): Promise<void> {
    if (this.#disposed) throw new Error('Shortcut capture is unavailable');
    if (this.#shortcutCaptureOwners.has(ownerWebContentsId)) return;
    this.#shortcutCaptureOwners.add(ownerWebContentsId);
    try {
      await this.#serializeTransaction(() => this.#syncActivation());
    } catch (error: unknown) {
      // Keep the owner active so a stale native activation cannot start dictation while the
      // renderer reports the capture failure. Blur/destruction releases it and retries sync.
      this.requestSync();
      throw error;
    }
  }

  async endShortcutCapture(ownerWebContentsId: number): Promise<void> {
    if (!this.#shortcutCaptureOwners.delete(ownerWebContentsId) || this.#disposed) return;
    try {
      await this.#serializeTransaction(() => this.#syncActivation());
    } catch (error: unknown) {
      this.requestSync();
      throw error;
    }
  }

  requestSync(): void {
    if (this.#disposed) return;
    this.#syncRequested = true;
    if (this.#transactionActive || this.#syncScheduled) return;
    this.#syncScheduled = true;
    void this.#serializeTransaction(() => this.#drainSyncRequests()).then(
      () => {
        this.#syncScheduled = false;
        if (this.#syncRequested) this.requestSync();
      },
      () => {
        // Retain the failed request for the next readiness/settings signal without hot-looping.
        this.#syncScheduled = false;
      },
    );
  }

  updateGeneral(patch: PublicSettingsPatch): Promise<Settings> {
    return this.#serializeTransaction(async () => {
      const current = this.#settings.get();
      const nextEnabled = patch.app?.enabled ?? current.app.enabled;
      try {
        if (this.#helper.readiness.status === 'ready') {
          await this.#helper.configureActivation(
            this.#activationEnabled(nextEnabled),
            profileBindings(current.dictationProfiles),
          );
        }
        return await this.#settings.update(patch);
      } catch (error: unknown) {
        await this.#syncActivation().catch(() => this.requestSync());
        throw error;
      }
    });
  }

  createProfile(input: DictationProfileCreate): Promise<Settings> {
    return this.#serializeTransaction(() => {
      const profile = { id: randomUUID(), ...DictationProfileCreateSchema.parse(input) };
      return this.#replaceProfiles([...this.#settings.get().dictationProfiles, profile]);
    });
  }

  updateProfile(id: string, patch: DictationProfilePatch): Promise<Settings> {
    return this.#serializeTransaction(() => {
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
    return this.#serializeTransaction(() => {
      if (BuiltInDictationProfileIdSchema.safeParse(id).success) {
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
    return this.#serializeTransaction(() => {
      const replacement = builtInDictationProfile(id);
      if (replacement === null) throw new Error('Only built-in profiles can be reset');
      return this.#replaceProfiles(
        this.#settings
          .get()
          .dictationProfiles.map((profile) => (profile.id === id ? replacement : profile)),
      );
    });
  }

  async #replaceProfiles(input: readonly DictationProfile[]): Promise<Settings> {
    const profiles = DictationProfileListSchema.parse(input);
    const current = this.#settings.get();
    try {
      if (this.#helper.readiness.status === 'ready') {
        await this.#helper.configureActivation(
          this.#activationEnabled(current.app.enabled),
          profileBindings(profiles),
        );
      }
      return await this.#settings.update({ dictationProfiles: profiles });
    } catch (error: unknown) {
      await this.#syncActivation().catch(() => this.requestSync());
      throw error;
    }
  }

  #activationEnabled(enabled: boolean): boolean {
    return enabled && this.#isModelReady() && !this.shortcutCaptureActive;
  }

  async #syncActivation(): Promise<void> {
    if (this.#helper.readiness.status !== 'ready') return;
    const settings = this.#settings.get();
    try {
      await this.#helper.configureActivation(
        this.#activationEnabled(settings.app.enabled),
        profileBindings(settings.dictationProfiles),
      );
    } catch (error: unknown) {
      this.#onSyncFailure(error);
      throw error;
    }
  }

  async #drainSyncRequests(): Promise<void> {
    while (this.#syncRequested && !this.#disposed) {
      this.#syncRequested = false;
      try {
        await this.#syncActivation();
      } catch (error: unknown) {
        this.#syncRequested = true;
        throw error;
      }
    }
  }

  #serializeTransaction<Result>(operation: () => Promise<Result>): Promise<Result> {
    const execute = async () => {
      this.#transactionActive = true;
      try {
        const value = await operation();
        try {
          await this.#drainSyncRequests();
        } catch {
          // The operation already succeeded. A redundant subscription-driven reconciliation must
          // not report a committed mutation as failed; requestSync retains and retries it.
        }
        return value;
      } finally {
        this.#transactionActive = false;
        if (this.#syncRequested && !this.#syncScheduled) this.requestSync();
      }
    };
    const result = this.#transactionTail.then(execute, execute);
    this.#transactionTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function profileBindings(profiles: readonly DictationProfile[]): ActivationBinding[] {
  return profiles.map((profile) => ({ profileId: profile.id, shortcut: profile.shortcut }));
}
