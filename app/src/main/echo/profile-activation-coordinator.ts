import { randomUUID } from 'node:crypto';
import {
  BuiltInDictationProfileIdSchema,
  DictationProfileCreateSchema,
  DictationProfileListSchema,
  DictationProfilePatchSchema,
  builtInDictationProfile,
  isReservedBindingForProfile,
  RESERVED_DICTATION_BINDING_ERROR,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../shared/schemas/dictation-profiles';
import type { ActivationBinding } from '../../shared/helper/protocol';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
import type { ShortcutCaptureLeaseId } from '../../shared/schemas/shortcut-capture';
import type { SettingsStore } from '../persistence/settings-store';
import type { EchoHelperPort } from './echo-session-ports';

const SYNC_RETRY_DELAYS_MS = [250, 1_000, 4_000, 15_000, 30_000] as const;

export class ProfileActivationCoordinator {
  readonly #settings: SettingsStore;
  readonly #helper: EchoHelperPort;
  readonly #isModelReady: () => boolean;
  readonly #onSyncFailure: (error: unknown) => void;
  readonly #onSyncSuccess: () => void;
  #transactionTail: Promise<void> = Promise.resolve();
  #transactionActive = false;
  #syncRequested = false;
  #syncScheduled = false;
  #syncRetryTimer: ReturnType<typeof setTimeout> | null = null;
  #syncRetryAttempt = 0;
  #syncRetryGeneration = 0;
  readonly #shortcutCaptureLeases = new Set<ShortcutCaptureLeaseId>();
  #disposed = false;

  constructor(options: {
    readonly settings: SettingsStore;
    readonly helper: EchoHelperPort;
    readonly isModelReady: () => boolean;
    readonly onSyncFailure?: (error: unknown) => void;
    readonly onSyncSuccess?: () => void;
  }) {
    this.#settings = options.settings;
    this.#helper = options.helper;
    this.#isModelReady = options.isModelReady;
    this.#onSyncFailure = options.onSyncFailure ?? (() => undefined);
    this.#onSyncSuccess = options.onSyncSuccess ?? (() => undefined);
  }

  dispose(): void {
    this.#disposed = true;
    this.#syncRequested = false;
    this.#cancelSyncRetry();
    this.#shortcutCaptureLeases.clear();
  }

  get shortcutCaptureActive(): boolean {
    return this.#shortcutCaptureLeases.size > 0;
  }

  async beginShortcutCapture(leaseId: ShortcutCaptureLeaseId): Promise<void> {
    if (this.#disposed) throw new Error('Shortcut capture is unavailable');
    if (this.#shortcutCaptureLeases.has(leaseId)) return;
    this.#shortcutCaptureLeases.add(leaseId);
    try {
      await this.#serializeTransaction(() => this.#syncActivation());
    } catch (error: unknown) {
      // Keep the owner active so a stale native activation cannot start dictation while the
      // renderer reports the capture failure. Blur/destruction releases it and retries sync.
      this.requestSync();
      throw error;
    }
  }

  async endShortcutCapture(leaseId: ShortcutCaptureLeaseId): Promise<void> {
    if (!this.#shortcutCaptureLeases.delete(leaseId) || this.#disposed) return;
    await this.retryShortcutCaptureRestoration();
  }

  async retryShortcutCaptureRestoration(): Promise<void> {
    if (this.#disposed) return;
    try {
      await this.#serializeTransaction(() => this.#syncActivation());
    } catch (error: unknown) {
      this.requestSync();
      throw error;
    }
  }

  synchronize(): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    return this.#serializeTransaction(() => this.#syncActivation());
  }

  requestSync(): void {
    if (this.#disposed) return;
    this.#syncRequested = true;
    // A fresh readiness or settings signal supersedes an old delay and gets one immediate attempt.
    if (this.#syncRetryTimer !== null) this.#cancelSyncRetry();
    this.#scheduleSyncDrain();
  }

  #scheduleSyncDrain(): void {
    if (this.#disposed || this.#transactionActive || this.#syncScheduled) return;
    this.#syncScheduled = true;
    void this.#serializeTransaction(() => this.#drainSyncRequests()).then(
      () => {
        this.#syncScheduled = false;
        this.#syncRetryAttempt = 0;
        if (this.#syncRequested) this.#scheduleSyncDrain();
      },
      () => {
        this.#syncScheduled = false;
        this.#scheduleSyncRetry();
      },
    );
  }

  #scheduleSyncRetry(): void {
    if (
      this.#disposed ||
      !this.#syncRequested ||
      this.#helper.readiness.status !== 'ready' ||
      this.#syncRetryTimer !== null
    ) {
      return;
    }
    const delayIndex = Math.min(this.#syncRetryAttempt, SYNC_RETRY_DELAYS_MS.length - 1);
    const delay = SYNC_RETRY_DELAYS_MS[delayIndex] ?? SYNC_RETRY_DELAYS_MS[0];
    this.#syncRetryAttempt += 1;
    const generation = ++this.#syncRetryGeneration;
    this.#syncRetryTimer = setTimeout(() => {
      if (this.#disposed || generation !== this.#syncRetryGeneration) return;
      this.#syncRetryTimer = null;
      if (this.#helper.readiness.status === 'ready') this.#scheduleSyncDrain();
    }, delay);
    this.#syncRetryTimer.unref();
  }

  #cancelSyncRetry(): void {
    this.#syncRetryGeneration += 1;
    if (this.#syncRetryTimer !== null) clearTimeout(this.#syncRetryTimer);
    this.#syncRetryTimer = null;
    this.#syncRetryAttempt = 0;
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
      if (parsed.shortcut !== undefined && isReservedBindingForProfile(id, parsed.shortcut)) {
        throw new Error(RESERVED_DICTATION_BINDING_ERROR);
      }
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

  replaceProfiles(input: readonly DictationProfile[]): Promise<Settings> {
    return this.#serializeTransaction(() => this.#replaceProfiles(input));
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
      if (this.#syncRetryTimer !== null) this.#cancelSyncRetry();
      this.#onSyncSuccess();
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
        if (
          this.#syncRequested &&
          !this.#syncScheduled &&
          this.#syncRetryTimer === null &&
          !this.#disposed
        ) {
          this.#scheduleSyncDrain();
        }
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
