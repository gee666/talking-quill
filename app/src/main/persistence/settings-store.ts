import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  SettingsPatchSchema,
  SettingsSchema,
  type Settings,
  type SettingsPatch,
} from '../../shared/schemas/settings';
import { GENERAL_PROFILE_ID } from '../../shared/schemas/dictation-profiles';
import { preserveInvalidFile, readUtf8File, writeJsonAtomic } from './atomic-json';

export type SettingsMigration = (input: Readonly<Record<string, unknown>>) => unknown;
export type SettingsMigrations = Readonly<Record<number, SettingsMigration>>;
export type SettingsListener = (settings: Settings) => void;

export interface SettingsStoreIo {
  readonly read: (path: string) => Promise<string | null>;
  readonly write: (path: string, value: unknown) => Promise<void>;
  readonly preserveInvalid: (path: string, previouslyReadSource?: string) => Promise<string | null>;
}

export interface SettingsStoreOptions {
  readonly migrations?: SettingsMigrations;
  readonly io?: SettingsStoreIo;
}

export type SettingsDiagnostic =
  | {
      readonly code: 'INVALID_SETTINGS_RECOVERED';
      readonly reason: 'parse' | 'schema' | 'migration';
      readonly preservedAt: string | null;
    }
  | {
      readonly code: 'UNSUPPORTED_SETTINGS_VERSION';
      readonly foundVersion: number;
    }
  | {
      readonly code: 'SETTINGS_IO_ERROR';
      readonly operation: 'read' | 'write' | 'preserve-invalid';
    };

const DEFAULT_IO: SettingsStoreIo = Object.freeze({
  read: readUtf8File,
  write: writeJsonAtomic,
  preserveInvalid: preserveInvalidFile,
});

export class UnsupportedSettingsVersionError extends Error {
  readonly foundVersion: number;

  constructor(foundVersion: number) {
    super('The settings file was created by a newer application version');
    this.name = 'UnsupportedSettingsVersionError';
    this.foundVersion = foundVersion;
  }
}

export class SettingsStore {
  readonly #path: string;
  readonly #migrations: SettingsMigrations;
  readonly #io: SettingsStoreIo;
  readonly #listeners = new Set<SettingsListener>();
  readonly #pendingWrites = new Set<Promise<Settings>>();
  readonly #unflushedFailures: unknown[] = [];
  #settings: Settings = structuredClone(DEFAULT_SETTINGS);
  #writeQueue: Promise<void> = Promise.resolve();
  #diagnostic: SettingsDiagnostic | null = null;

  constructor(path: string, options: SettingsStoreOptions = {}) {
    this.#path = path;
    this.#migrations = options.migrations ?? Object.freeze({});
    this.#io = options.io ?? DEFAULT_IO;
  }

  async initialize(): Promise<void> {
    let source: string | null;
    try {
      source = await this.#io.read(this.#path);
    } catch (error: unknown) {
      this.#setIoDiagnostic('read');
      throw error;
    }
    this.#clearIoDiagnostic();

    if (source === null) {
      await this.#persistInitial(this.#settings);
      return;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse(source) as unknown;
    } catch {
      await this.#recoverCorruptSettings('parse', source);
      return;
    }

    const sourceVersion = readSchemaVersion(parsed);
    if (typeof sourceVersion === 'number' && sourceVersion > SETTINGS_SCHEMA_VERSION) {
      this.#diagnostic = Object.freeze({
        code: 'UNSUPPORTED_SETTINGS_VERSION',
        foundVersion: sourceVersion,
      });
      throw new UnsupportedSettingsVersionError(sourceVersion);
    }

    let migrated: unknown;
    try {
      migrated = this.#migrate(parsed);
    } catch {
      await this.#recoverCorruptSettings('migration', source);
      return;
    }

    const validated = SettingsSchema.safeParse(migrated);
    if (!validated.success) {
      await this.#recoverCorruptSettings('schema', source);
      return;
    }

    this.#settings = validated.data;
    if (migrated !== parsed) await this.#persistInitial(this.#settings);
  }

  get(): Settings {
    return structuredClone(this.#settings);
  }

  getDiagnostic(): SettingsDiagnostic | null {
    return this.#diagnostic === null ? null : { ...this.#diagnostic };
  }

  getWarning(): SettingsDiagnostic | null {
    return this.getDiagnostic();
  }

  async update(patch: SettingsPatch, signal?: AbortSignal): Promise<Settings> {
    const validatedPatch = SettingsPatchSchema.parse(patch);
    const update = this.#writeQueue.then(async () => {
      throwIfUpdateAborted(signal);
      const previous = this.#settings;
      const next = SettingsSchema.parse(mergeSettings(previous, validatedPatch));
      try {
        await this.#io.write(this.#path, next);
      } catch (error: unknown) {
        this.#setIoDiagnostic('write');
        throw error;
      }
      if (signal?.aborted === true) {
        try {
          await this.#io.write(this.#path, previous);
        } catch (error: unknown) {
          this.#setIoDiagnostic('write');
          throw error;
        }
        this.#clearIoDiagnostic();
        throw abortError();
      }
      this.#clearIoDiagnostic();
      this.#settings = next;
      const snapshot = this.get();
      for (const listener of this.#listeners) {
        try {
          listener(snapshot);
        } catch {
          // A failed notification must not turn a committed write into a persistence failure.
        }
      }
      return snapshot;
    });
    this.#pendingWrites.add(update);
    this.#writeQueue = update.then(
      () => undefined,
      () => undefined,
    );
    void update.then(
      () => this.#pendingWrites.delete(update),
      (error: unknown) => {
        this.#pendingWrites.delete(update);
        if (!isAbortError(error) && this.#unflushedFailures.length === 0) {
          this.#unflushedFailures.push(error);
        }
      },
    );
    return update;
  }

  subscribe(listener: SettingsListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  async flush(): Promise<void> {
    let firstFailure: unknown;
    let failed = false;
    while (this.#pendingWrites.size > 0 || this.#unflushedFailures.length > 0) {
      if (this.#unflushedFailures.length > 0) {
        if (!failed) firstFailure = this.#unflushedFailures[0];
        failed = true;
        this.#unflushedFailures.length = 0;
      }
      if (this.#pendingWrites.size > 0) {
        await Promise.allSettled([...this.#pendingWrites]);
      }
    }
    if (failed) throw firstFailure;
  }

  async #persistInitial(settings: Settings): Promise<void> {
    try {
      await this.#io.write(this.#path, settings);
      this.#clearIoDiagnostic();
    } catch (error: unknown) {
      this.#setIoDiagnostic('write');
      throw error;
    }
  }

  async #recoverCorruptSettings(
    reason: 'parse' | 'schema' | 'migration',
    source: string,
  ): Promise<void> {
    let preservedAt: string | null;
    try {
      preservedAt = await this.#io.preserveInvalid(this.#path, source);
    } catch (error: unknown) {
      this.#setIoDiagnostic('preserve-invalid');
      throw error;
    }

    this.#diagnostic = Object.freeze({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason,
      preservedAt,
    });
    this.#settings = structuredClone(DEFAULT_SETTINGS);
    await this.#persistInitial(this.#settings);
  }

  #setIoDiagnostic(operation: 'read' | 'write' | 'preserve-invalid'): void {
    this.#diagnostic = Object.freeze({ code: 'SETTINGS_IO_ERROR', operation });
  }

  #clearIoDiagnostic(): void {
    if (this.#diagnostic?.code === 'SETTINGS_IO_ERROR') this.#diagnostic = null;
  }

  #migrate(input: unknown): unknown {
    if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
    let current: unknown = input;
    let version: unknown = readSchemaVersion(input);
    while (typeof version === 'number' && version < SETTINGS_SCHEMA_VERSION) {
      const migration = this.#migrations[version];
      if (migration === undefined) throw new Error('Unsupported settings version');
      const previousVersion = version;
      current = migration(current as Readonly<Record<string, unknown>>);
      if (typeof current !== 'object' || current === null) throw new Error('Invalid migration');
      version = readSchemaVersion(current);
      if (typeof version !== 'number' || version <= previousVersion) {
        throw new Error('Settings migration did not advance its version');
      }
    }
    return current;
  }
}

function throwIfUpdateAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted === true) throw abortError();
}

function abortError(): DOMException {
  return new DOMException('The settings update was cancelled.', 'AbortError');
}

function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}

function readSchemaVersion(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return undefined;
  return (input as Readonly<Record<string, unknown>>).schemaVersion;
}

export function mergeSettings(current: Settings, patch: SettingsPatch): Settings {
  const providerPatches = patch.smartProcessing?.providers ?? {};
  const providers = { ...current.smartProcessing.providers };
  for (const [providerId, providerPatch] of Object.entries(providerPatches)) {
    providers[providerId as keyof typeof providers] = {
      ...providers[providerId as keyof typeof providers],
      ...providerPatch,
    };
  }
  for (const [providerId, replacement] of Object.entries(
    patch.smartProcessing?.providerReplacements ?? {},
  )) {
    providers[providerId as keyof typeof providers] = replacement;
  }
  const dictationProfiles = patch.dictationProfiles ?? current.dictationProfiles;
  const general = dictationProfiles.find((profile) => profile.id === GENERAL_PROFILE_ID);
  if (general === undefined) throw new Error('The General dictation profile is required');
  const currentEvidence = current.welcome.activationEvidence;
  const evidenceProfile =
    currentEvidence === null || currentEvidence === undefined
      ? undefined
      : dictationProfiles.find(
          (profile) =>
            profile.id === currentEvidence.profileId &&
            profile.activationKey === currentEvidence.activationKey &&
            profile.shift === currentEvidence.shift,
        );
  const bindingEvidenceInvalidated =
    patch.dictationProfiles !== undefined &&
    currentEvidence !== null &&
    currentEvidence !== undefined &&
    evidenceProfile === undefined;
  return SettingsSchema.parse({
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    app: {
      ...current.app,
      ...patch.app,
      activationKey: general.activationKey,
      defaultProcessingMode: general.processingMode,
    },
    recording: { ...current.recording, ...patch.recording },
    transcription: { ...current.transcription, ...patch.transcription },
    dictationProfiles,
    privacy: { ...current.privacy, ...patch.privacy },
    smartProcessing: {
      selectedProviderId:
        patch.smartProcessing?.selectedProviderId ?? current.smartProcessing.selectedProviderId,
      providers,
      credentialEpochs: {
        ...current.smartProcessing.credentialEpochs,
        ...patch.smartProcessing?.credentialEpochs,
      },
      piInstallationPath:
        patch.smartProcessing?.piInstallationPath !== undefined
          ? patch.smartProcessing.piInstallationPath
          : current.smartProcessing.piInstallationPath,
      onScreenAwarenessEnabled:
        patch.smartProcessing?.onScreenAwarenessEnabled ??
        current.smartProcessing.onScreenAwarenessEnabled,
      visionOverrides:
        patch.smartProcessing?.visionOverrides ?? current.smartProcessing.visionOverrides,
    },
    voiceCommands: patch.voiceCommands ?? current.voiceCommands,
    customVocabulary: patch.customVocabulary ?? current.customVocabulary,
    welcome: {
      ...current.welcome,
      ...patch.welcome,
      ...(bindingEvidenceInvalidated
        ? {
            activationTested: false,
            activationEvidence: null,
            completedAt: current.welcome.completedAt,
            lastStep:
              current.welcome.completedAt === null
                ? Math.min(current.welcome.lastStep, 4)
                : current.welcome.lastStep,
            revision: (current.welcome.revision ?? 0) + 1,
          }
        : {}),
    },
  });
}
