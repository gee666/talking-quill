import { randomUUID } from 'node:crypto';
import {
  ECHO_ACTIVATION_TEST_TIMEOUT_MS,
  ECHO_HOLD_THRESHOLD_MS,
  ECHO_LEVEL_EVENT_INTERVAL_MS,
  ECHO_TERMINAL_DISPLAY_MS,
} from '../../shared/constants/echo-session';
import { PCM_FRAME_DURATION_MS, SESSION_CAP_MS } from '../../shared/constants/audio';
import type { ActivationBinding, HelperNotification } from '../../shared/helper/protocol';
import {
  ActivationTestStateSchema,
  IDLE_ACTIVATION_TEST,
  type ActivationTestState,
} from '../../shared/schemas/activation-test';
import {
  EchoSessionSnapshotSchema,
  type EchoAbortReason,
  type EchoSessionSnapshot,
  type PiFallbackCategory,
} from '../../shared/schemas/echo-session';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import { TranscriptionResultSchema } from '../../shared/schemas/transcription';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
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
import type { VoiceCommand, VoiceCommandMatch } from '../../shared/schemas/commands';
import type {
  WhisperStreamingSession,
  WhisperWorkerClient,
} from '../transcription/whisper-worker-client';
import type { HelperClient } from '../helper';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import { SilencePolicy } from '../audio/silence-policy';
import type { RecordingService } from '../audio/recording-service';
import type { SessionHistoryRecord } from '../history/session-history-mapper';
import type { InsertionService } from '../insertion/insertion-service';
import type { WindowManager } from '../app/window-manager';
import { ProviderError } from '../providers/errors';
import type {
  FrozenSmartTranscriptSession,
  SmartTranscriptProcessor as ProductionSmartTranscriptProcessor,
} from '../smart/smart-transcription-service';
import {
  IDLE_ECHO_SESSION,
  reduceEchoSession,
  type EchoSessionEffect,
  type EchoSessionEvent,
  type EchoSessionState,
} from './session-reducer';

const EXTENDED_PUSH_SAMPLES = 160_000;
const MAX_EXTENDED_BUFFERED_SAMPLES = EXTENDED_PUSH_SAMPLES * 2;
const INSERTION_CONTROLLER_TIMEOUT_MS = 5_000;
const MODEL_USE_SETTLE_TIMEOUT_MS = 1_000;
const STREAM_CANCEL_TIMEOUT_MS = 1_000;
const FIRST_AUDIO_TIMEOUT_MS = 1_000;

export type EchoHelperPort = Pick<
  HelperClient,
  | 'readiness'
  | 'subscribeNotifications'
  | 'subscribeReadiness'
  | 'setSessionCapture'
  | 'getFrontApp'
  | 'resetSessionCapture'
> & {
  configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]): Promise<unknown>;
};
export type EchoRecordingPort = Pick<RecordingService, 'startDictation' | 'stopDictation'>;
export type EchoWhisperPort = Pick<WhisperWorkerClient, 'transcribe' | 'startSession'>;
export type EchoInsertionPort = Pick<InsertionService, 'insert'>;
export interface EchoHistoryPort {
  record(outcome: SessionHistoryRecord): boolean;
}

export interface EchoModelUseGrant {
  readonly status: { readonly state: string };
  release(): void;
}

export interface LegacySmartTranscriptProcessor {
  process(text: string, signal: AbortSignal): Promise<string>;
}
export type SmartTranscriptProcessor =
  ProductionSmartTranscriptProcessor | LegacySmartTranscriptProcessor;

export interface VoiceCommandMatcherPort {
  match(transcript: string): VoiceCommandMatch | null;
}

export class EchoSessionController {
  readonly #settings: SettingsStore;
  readonly #recording: EchoRecordingPort;
  readonly #whisper: EchoWhisperPort;
  readonly #helper: EchoHelperPort;
  readonly #insertion: EchoInsertionPort;
  readonly #history: EchoHistoryPort | null;
  readonly #windows: WindowManager;
  readonly #events: IpcEventEmitter;
  readonly #smart: SmartTranscriptProcessor | null;
  readonly #commands: VoiceCommandMatcherPort | null;
  readonly #sound: () => void;
  #appPreferences: Settings['app'];
  readonly #isModelReady: () => boolean;
  readonly #acquireModelUse: (
    modelId: WhisperModelId,
    signal: AbortSignal,
  ) => Promise<EchoModelUseGrant>;
  readonly #listeners = new Set<(snapshot: EchoSessionSnapshot) => void>();
  readonly #removeHelperNotifications: () => void;
  readonly #removeHelperReadiness: () => void;
  readonly #removeSettings: () => void;
  #state: EchoSessionState = IDLE_ECHO_SESSION;
  #abort: AbortController | null = null;
  #captureId: string | null = null;
  #captureStopping = false;
  #pcmChunks: Float32Array[] = [];
  #totalSamples = 0;
  #streamedSamples = 0;
  #discardedSamples = 0;
  #streamPushPending = false;
  #stream: WhisperStreamingSession | null = null;
  #streamOpening: Promise<WhisperStreamingSession> | null = null;
  #streamTail: Promise<void> = Promise.resolve();
  #streamFailure: unknown = null;
  #modelUse: EchoModelUseGrant | null = null;
  #modelUseOpening: Promise<void> = Promise.resolve();
  #effectTail: Promise<void> = Promise.resolve();
  #holdTimer: ReturnType<typeof setTimeout> | null = null;
  #capTimer: ReturnType<typeof setTimeout> | null = null;
  #audioStartTimer: ReturnType<typeof setTimeout> | null = null;
  #resetTimer: ReturnType<typeof setTimeout> | null = null;
  #silence: SilencePolicy | null = null;
  #pendingSilenceSubmit = false;
  #lastLevelAt = 0;
  #activationTail: Promise<void> = Promise.resolve();
  #activationSyncRequested = false;
  #activationSyncScheduled = false;
  #captureGeneration = 0;
  #helperCaptureDesired = false;
  #helperCaptureApplied: boolean | null = false;
  #helperCaptureRevision = 0;
  #helperCaptureRunning = false;
  #helperCaptureTail: Promise<void> = Promise.resolve();
  #captureOffGuaranteed = true;
  #teardownComplete = true;
  #activationTest: ActivationTestState = IDLE_ACTIVATION_TEST;
  #activationTestOwner: number | null = null;
  #removeActivationTestOwner: (() => void) | null = null;
  #activationTestHoldTimer: ReturnType<typeof setTimeout> | null = null;
  #activationTestExpiryTimer: ReturnType<typeof setTimeout> | null = null;
  #activationTestPressedAt: number | null = null;
  #historyWrittenSessionId: string | null = null;
  #sessionHistoryAllowed = false;
  #sessionSettings: Readonly<Settings> | null = null;
  #sessionProfile: Readonly<DictationProfile> | null = null;
  #sessionProviderId: string | null = null;
  #sessionModelId: string | null = null;
  #sessionVoiceCommand: VoiceCommand | null = null;
  #smartSession: FrozenSmartTranscriptSession | null = null;
  #sessionScreenshotFilename: string | null = null;
  #profileTransaction: Promise<void> = Promise.resolve();
  #disposed = false;

  constructor(options: {
    readonly settings: SettingsStore;
    readonly recording: EchoRecordingPort;
    readonly whisper: EchoWhisperPort;
    readonly helper: EchoHelperPort;
    readonly insertion: EchoInsertionPort;
    readonly history?: EchoHistoryPort;
    readonly windows: WindowManager;
    readonly events: IpcEventEmitter;
    readonly smartProcessor?: SmartTranscriptProcessor;
    readonly commands?: VoiceCommandMatcherPort;
    readonly sound?: () => void;
    readonly isModelReady?: () => boolean;
    readonly acquireModelUse?: (
      modelId: WhisperModelId,
      signal: AbortSignal,
    ) => Promise<EchoModelUseGrant>;
  }) {
    this.#settings = options.settings;
    this.#appPreferences = this.#settings.get().app;
    this.#recording = options.recording;
    this.#whisper = options.whisper;
    this.#helper = options.helper;
    this.#insertion = options.insertion;
    this.#history = options.history ?? null;
    this.#windows = options.windows;
    this.#events = options.events;
    this.#smart = options.smartProcessor ?? null;
    this.#commands = options.commands ?? null;
    this.#sound = options.sound ?? (() => undefined);
    this.#isModelReady = options.isModelReady ?? (() => true);
    this.#acquireModelUse =
      options.acquireModelUse ??
      (() => Promise.resolve({ status: { state: 'ready' }, release: () => undefined }));
    this.#removeHelperNotifications = this.#helper.subscribeNotifications((notification) =>
      this.acceptHelperNotification(notification),
    );
    this.#removeHelperReadiness = this.#helper.subscribeReadiness((readiness) => {
      if (readiness.status === 'ready') {
        this.#queueActivationSync();
        void this.#setHelperCaptureDesired(
          isCapturePhase(this.#state.phase),
          this.#captureGeneration,
        );
      } else {
        this.#helperCaptureApplied = null;
        void this.#setHelperCaptureDesired(false, this.#captureGeneration);
        if (this.#activationTest.active) this.#stopActivationTest();
        if (this.#state.phase !== 'idle') this.abort('target-lost');
      }
    });
    this.#removeSettings = this.#settings.subscribe((next) => {
      this.#appPreferences = next.app;
      this.#queueActivationSync();
      // Privacy grants are frozen at session start. Revocation applies immediately; enabling
      // history while a session is active affects only a future session.
      if (!next.privacy.historyEnabled) this.#sessionHistoryAllowed = false;
    });
  }

  get activationTestState(): ActivationTestState {
    return this.#activationTest;
  }

  get snapshot(): EchoSessionSnapshot {
    return EchoSessionSnapshotSchema.parse({
      sessionId: this.#state.sessionId,
      phase: this.#state.phase,
      dictationMode: this.#state.dictationMode,
      processingMode: this.#state.processingMode,
      alternate: this.#state.alternate,
      rms: this.#state.rms,
      elapsedMs: this.#state.elapsedMs,
      transcript: this.#state.transcript,
      abortReason: this.#state.abortReason,
      fallbackCategory: this.#state.fallbackCategory,
      completion: this.#state.completion,
      message: this.#state.message,
    });
  }

  subscribe(listener: (snapshot: EchoSessionSnapshot) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  initialize(): void {
    this.#queueActivationSync();
    this.#publish();
  }

  startActivationTest(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
  ): ActivationTestState {
    const unavailableReason = !this.#settings.get().app.enabled
      ? 'app-disabled'
      : this.#state.phase !== 'idle'
        ? 'session-active'
        : this.#helper.readiness.status !== 'ready'
          ? 'helper-unavailable'
          : null;
    if (unavailableReason !== null) {
      this.#activationTest = {
        ...IDLE_ACTIVATION_TEST,
        unavailableReason,
      };
      this.#publishActivationTest();
      return this.#activationTest;
    }
    this.#stopActivationTest();
    this.#activationTestOwner = ownerWebContentsId;
    this.#removeActivationTestOwner = onDestroyed(() =>
      this.stopActivationTest(ownerWebContentsId),
    );
    this.#activationTest = ActivationTestStateSchema.parse({
      active: true,
      phase: 'waiting',
      profileId: null,
      activationKey: null,
      shift: false,
      elapsedMs: 0,
      unavailableReason: null,
    });
    this.#activationTestExpiryTimer = setTimeout(
      () => this.#stopActivationTest(),
      ECHO_ACTIVATION_TEST_TIMEOUT_MS,
    );
    this.#activationTestExpiryTimer.unref();
    this.#publishActivationTest();
    return this.#activationTest;
  }

  stopActivationTest(ownerWebContentsId?: number): ActivationTestState {
    if (ownerWebContentsId !== undefined && this.#activationTestOwner !== ownerWebContentsId) {
      return this.#activationTest;
    }
    this.#stopActivationTest();
    return this.#activationTest;
  }

  acceptHelperNotification(notification: HelperNotification): void {
    this.#onHelperNotification(notification);
  }

  stop(): void {
    if (this.#state.phase === 'recordingQuick' || this.#state.phase === 'recordingExtended') {
      this.#dispatch({ type: 'submit', source: 'stop' });
    }
  }

  cancel(): void {
    this.abort('user-cancel');
  }

  abort(reason: EchoAbortReason, fallbackCategory?: PiFallbackCategory): void {
    const rawFallback =
      (reason === 'provider-error' || reason === 'timeout') &&
      this.#state.phase === 'processingSmart' &&
      this.#state.transcript !== null;
    this.#abort?.abort();
    if (rawFallback) this.#abort = new AbortController();
    this.#dispatch({
      type: 'abort',
      reason,
      ...(fallbackCategory === undefined ? {} : { fallbackCategory }),
    });
  }

  readinessChanged(): void {
    this.#queueActivationSync();
  }

  async updateGeneral(patch: PublicSettingsPatch): Promise<Settings> {
    const current = this.#settings.get();
    const nextEnabled = patch.app?.enabled ?? current.app.enabled;
    if (!nextEnabled && this.#state.phase !== 'idle') this.cancel();
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
      if (!current.some((profile) => profile.id === id))
        throw new Error('Dictation profile not found');
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
      if (!current.some((profile) => profile.id === id))
        throw new Error('Dictation profile not found');
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
      // The settings subscription schedules an authoritative activation sync.
      // Keep it inside this transaction so a following CRUD operation cannot
      // interleave and leave the helper on an older profile set.
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

  async shutdown(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#abort?.abort();
    this.#stopActivationTest();
    this.#clearResetTimer();
    this.#dispatch({ type: 'abort', reason: 'shutdown' });
    await this.#effectTail.catch(() => undefined);
    await this.#teardown();
    this.#clearResetTimer();
    this.#removeSettings();
    this.#removeHelperReadiness();
    this.#removeHelperNotifications();
    this.#listeners.clear();
  }

  #onHelperNotification(notification: HelperNotification): void {
    if (this.#disposed || notification.method === 'paste.committed') return;
    if (notification.method === 'activation.event') {
      if (notification.params.phase === 'down') {
        // Native backends arm session-key capture before publishing activation.down. Mark the
        // applied state unknown so every rejection/test path explicitly confirms it disabled.
        this.#helperCaptureApplied = null;
        this.#captureOffGuaranteed = false;
      }
      if (this.#activationTest.active) {
        this.#onActivationTestNotification(notification);
        if (notification.params.phase === 'down') {
          void this.#setHelperCaptureDesired(false, this.#captureGeneration);
        }
        return;
      }
      if (notification.params.phase === 'down') {
        if (this.#state.phase === 'idle') {
          const settings = this.#settings.get();
          if (
            !settings.app.enabled ||
            !this.#isModelReady() ||
            this.#helper.readiness.status !== 'ready'
          ) {
            void this.#setHelperCaptureDesired(false, this.#captureGeneration);
            return;
          }
          const profile = settings.dictationProfiles.find(
            (candidate) =>
              candidate.activationKey === notification.params.key &&
              candidate.shift === notification.params.shift,
          );
          if (profile === undefined) {
            void this.#setHelperCaptureDesired(false, this.#captureGeneration);
            return;
          }
          const processingMode = profile.processingMode;
          this.#sessionSettings = settings;
          this.#sessionProfile = deepFreezeProfile(profile);
          this.#dispatch({
            type: 'shortcut-down',
            sessionId: randomUUID(),
            alternate: profile.shift,
            processingMode,
            now: Date.now(),
          });
        } else if (
          this.#state.phase === 'recordingQuick' ||
          this.#state.phase === 'recordingExtended'
        ) {
          this.#dispatch({ type: 'submit', source: 'shortcut' });
        } else {
          // Every activation down arms native Esc/Enter capture. Active phases which do not own
          // this new shortcut must explicitly disarm it instead of waiting for terminal reset.
          void this.#setHelperCaptureDesired(
            isCapturePhase(this.#state.phase),
            this.#captureGeneration,
          );
        }
      } else this.#dispatch({ type: 'shortcut-up', now: Date.now() });
      return;
    }
    if (notification.params.phase !== 'down') return;
    if (notification.params.key === 'escape') this.cancel();
    else this.#dispatch({ type: 'submit', source: 'enter' });
  }

  #onActivationTestNotification(
    notification: Extract<HelperNotification, { method: 'activation.event' }>,
  ): void {
    const now = Date.now();
    if (notification.params.phase === 'down') {
      const profile = this.#settings
        .get()
        .dictationProfiles.find(
          (candidate) =>
            candidate.activationKey === notification.params.key &&
            candidate.shift === notification.params.shift,
        );
      if (profile === undefined) return;
      this.#clearActivationTestHoldTimer();
      this.#activationTestPressedAt = now;
      this.#activationTest = {
        active: true,
        phase: 'pressed',
        profileId: profile.id,
        activationKey: profile.activationKey,
        shift: profile.shift,
        elapsedMs: 0,
        unavailableReason: null,
      };
      this.#activationTestHoldTimer = setTimeout(() => {
        if (!this.#activationTest.active || this.#activationTestPressedAt === null) return;
        this.#activationTest = {
          ...this.#activationTest,
          phase: 'extended',
          elapsedMs: Date.now() - this.#activationTestPressedAt,
        };
        this.#publishActivationTest();
      }, ECHO_HOLD_THRESHOLD_MS);
      this.#activationTestHoldTimer.unref();
      this.#publishActivationTest();
      return;
    }
    if (this.#activationTestPressedAt === null) return;
    const elapsedMs = Math.max(0, now - this.#activationTestPressedAt);
    const extended =
      this.#activationTest.phase === 'extended' || elapsedMs >= ECHO_HOLD_THRESHOLD_MS;
    this.#clearActivationTestHoldTimer();
    this.#activationTestPressedAt = null;
    this.#activationTest = {
      ...this.#activationTest,
      phase: extended ? 'extended' : 'quick',
      elapsedMs,
    };
    this.#publishActivationTest();
  }

  #dispatch(event: EchoSessionEvent): void {
    const previous = this.#state;
    const transition = reduceEchoSession(previous, event);
    if (transition.state === previous && transition.effects.length === 0) return;
    if (transition.state.phase === 'error' && previous.phase !== 'error') {
      // Failure effects are serialized behind the operation that failed. Abort that operation so
      // a non-cooperative worker promise cannot trap terminal teardown behind the effect tail.
      this.#abort?.abort();
    }
    if (transition.state.phase === 'transcribing' && this.#captureId === null) {
      this.#abort?.abort();
      this.#abort = new AbortController();
    }
    this.#state = transition.state;
    this.#manageTimers(previous, transition.state);
    this.#recordTerminalOutcome(previous, transition.state);
    this.#publish();
    for (const effect of transition.effects) this.#enqueueEffect(effect);
  }

  #manageTimers(previous: EchoSessionState, next: EchoSessionState): void {
    if (previous.phase === 'idle' && next.phase === 'arming') {
      this.#captureGeneration += 1;
      this.#abort = new AbortController();
      this.#captureStopping = false;
      this.#teardownComplete = false;
      this.#pcmChunks = [];
      this.#totalSamples = 0;
      this.#streamedSamples = 0;
      this.#discardedSamples = 0;
      this.#streamPushPending = false;
      this.#stream = null;
      this.#streamOpening = null;
      this.#streamTail = Promise.resolve();
      this.#streamFailure = null;
      this.#modelUse = null;
      this.#historyWrittenSessionId = null;
      const sessionSettings = this.#sessionSettings ?? this.#settings.get();
      this.#sessionSettings = sessionSettings;
      this.#sessionHistoryAllowed = sessionSettings.privacy.historyEnabled;
      this.#sessionVoiceCommand = null;
      this.#sessionScreenshotFilename = null;
      this.#smartSession?.cleanup();
      this.#smartSession = null;
      const selectedProviderId = sessionSettings.smartProcessing.selectedProviderId;
      this.#sessionProviderId = selectedProviderId;
      this.#sessionModelId =
        sessionSettings.smartProcessing.providers[selectedProviderId]?.modelId ?? null;
      if (this.#state.processingMode === 'smart' && this.#smart !== null) {
        try {
          this.#smartSession =
            'beginSession' in this.#smart
              ? this.#smart.beginSession(this.#sessionProfile ?? DEFAULT_GENERAL_PROFILE)
              : legacySmartSession(this.#smart, this.#sessionProviderId, this.#sessionModelId);
          this.#sessionProviderId = this.#smartSession.providerId;
          this.#sessionModelId = this.#smartSession.modelId;
        } catch {
          this.#smartSession = null;
        }
      }
      const modelId = sessionSettings.transcription.modelId;
      const signal = this.#operationSignal();
      this.#modelUseOpening = this.#acquireModelUse(modelId, signal).then((grant) => {
        if (signal.aborted || !isCapturePhase(this.#state.phase)) {
          grant.release();
          return;
        }
        if (grant.status.state !== 'ready') {
          grant.release();
          throw new Error('The selected transcription model is unavailable');
        }
        this.#modelUse = grant;
      });
      // The start-capture effect observes this promise. Attaching a rejection handler now keeps
      // synchronous model-loss races from becoming process-level unhandled rejections.
      void this.#modelUseOpening.catch(() => undefined);
      this.#pendingSilenceSubmit = false;
      this.#silence = new SilencePolicy({
        mode: 'quick',
        preset: sessionSettings.recording.silencePreset,
      });
      this.#holdTimer = setTimeout(
        () => this.#dispatch({ type: 'hold-elapsed', now: Date.now() }),
        ECHO_HOLD_THRESHOLD_MS,
      );
      this.#holdTimer.unref();
      this.#capTimer = setTimeout(
        () => this.#dispatch({ type: 'submit', source: 'duration-cap' }),
        SESSION_CAP_MS.extended,
      );
      this.#capTimer.unref();
    }
    if (previous.phase === 'arming' && next.phase === 'recordingQuick') {
      this.#clearHoldTimer();
      this.#replaceCapTimer(SESSION_CAP_MS.quick - next.elapsedMs);
      if (this.#pendingSilenceSubmit) {
        queueMicrotask(() => this.#dispatch({ type: 'submit', source: 'silence' }));
      }
    }
    if (next.phase === 'recordingExtended' && previous.phase !== 'recordingExtended') {
      this.#clearHoldTimer();
      this.#silence = null;
    }
    if (next.phase === 'transcribing' || next.phase === 'cancelled' || next.phase === 'error') {
      this.#clearRecordingTimers();
    }
  }

  #enqueueEffect(effect: EchoSessionEffect): void {
    const operation = async () => {
      const operationSignal = this.#abort?.signal ?? null;
      try {
        await this.#runEffect(effect);
      } catch (error: unknown) {
        const terminal =
          this.#disposed ||
          this.#state.phase === 'idle' ||
          this.#state.phase === 'completed' ||
          this.#state.phase === 'cancelled' ||
          this.#state.phase === 'error';
        if (
          error instanceof Error &&
          error.name === 'AbortError' &&
          (operationSignal?.aborted === true || terminal)
        ) {
          if (!terminal && effect.type === 'insert') {
            this.#dispatch({ type: 'insertion-cancelled' });
          }
          return;
        }
        if (!terminal) {
          this.#abort?.abort();
          this.#dispatch({ type: 'fail', message: publicSessionError(error) });
        }
      }
    };
    this.#effectTail = this.#effectTail.then(operation, operation);
  }

  async #runEffect(effect: EchoSessionEffect): Promise<void> {
    if (effect.type === 'start-capture') {
      const signal = this.#operationSignal();
      const generation = this.#captureGeneration;
      if (signal.aborted || !isCapturePhase(this.#state.phase)) return;
      // The widget renderer is preloaded. Show its truthful arming state before any device, model,
      // or helper round trip so the global shortcut always receives immediate visual feedback.
      this.#windows.showWidget(this.#appPreferences.widgetSize, null);
      // Model readiness and native key-capture confirmation are independent. Start both gates
      // immediately so a cold model lease does not add its latency to the helper round trip.
      const helperCaptureOpening = this.#setHelperCaptureDesired(true, generation);
      await raceWithAbort(Promise.all([this.#modelUseOpening, helperCaptureOpening]), signal);
      if (!isCapturePhase(this.#state.phase)) return;
      const capturePromise = this.#recording.startDictation({
        onFrame: (samples, rms) => this.#onFrame(samples, rms),
        onUnexpectedStop: () =>
          this.#dispatch({ type: 'fail', message: 'The microphone stopped unexpectedly.' }),
      });
      void capturePromise
        .then(async (capture) => {
          if (signal.aborted || !isCapturePhase(this.#state.phase)) {
            await this.#recording.stopDictation(capture.captureId).catch(() => undefined);
          }
        })
        .catch(() => undefined);
      const capture = await raceWithAbort(capturePromise, signal);
      if (!this.#captureStillCurrent(signal)) return;
      this.#captureId = capture.captureId;
      if (!this.#state.audioReady) {
        this.#audioStartTimer = setTimeout(() => {
          this.#audioStartTimer = null;
          if (generation !== this.#captureGeneration || this.#state.audioReady) return;
          if (!isCapturePhase(this.#state.phase)) return;
          this.#abort?.abort();
          this.#dispatch({ type: 'fail', message: 'The microphone did not provide audio.' });
        }, FIRST_AUDIO_TIMEOUT_MS);
        this.#audioStartTimer.unref();
      }

      // Audio and UI feedback follow confirmed microphone activation without entering the
      // helper's serial foreground-app path. Emit them before the reducer boundary so an early
      // first frame plus pending submit cannot suppress activation feedback.
      this.#playSound();
      this.#dispatch({ type: 'capture-started' });
      return;
    }
    if (effect.type === 'begin-extended-transcription') {
      await this.#ensureExtendedStream();
      this.#queueExtendedAudio(false);
      return;
    }
    if (effect.type === 'stop-and-transcribe') {
      await this.#stopCapture(this.#captureGeneration);
      const signal = this.#operationSignal();
      if (this.#state.processingMode === 'smart' && this.#smartSession !== null) {
        await raceWithAbort(this.#smartSession.prepare(signal), signal);
      }
      const text = await raceWithAbort(this.#transcribe(), signal);
      const match = this.#commands?.match(text) ?? null;
      if (match !== null) {
        this.#smartSession?.cleanup();
        this.#smartSession = null;
        this.#sessionVoiceCommand = match.command;
        this.#dispatch({ type: 'voice-command-matched', transcript: text, command: match.command });
      } else {
        this.#dispatch({
          type: 'transcribed',
          text,
          smart: this.#state.processingMode === 'smart',
        });
      }
      return;
    }
    if (effect.type === 'process-smart') {
      if (this.#smartSession === null) {
        this.#dispatch({ type: 'abort', reason: 'provider-error' });
        return;
      }
      const signal = this.#operationSignal();
      try {
        const result = await raceWithAbort(this.#smartSession.process(effect.text, signal), signal);
        this.#sessionScreenshotFilename = result.screenshotFilename;
        this.#dispatch({ type: 'smart-completed', text: result.text });
      } catch (error: unknown) {
        this.#smartSession.cleanup();
        this.#sessionScreenshotFilename = null;
        if (!signal.aborted && this.#state.phase === 'processingSmart') {
          const reason =
            error instanceof ProviderError && error.code === 'TIMEOUT'
              ? 'timeout'
              : 'provider-error';
          this.abort(reason, piFallbackCategory(this.#smartSession.providerId, error));
        }
      }
      return;
    }
    if (effect.type === 'insert') {
      const signal = this.#operationSignal();
      let acceptsCommit = true;
      try {
        const result = await withDeadline(
          this.#insertion.insert(effect.text, signal, () => {
            if (acceptsCommit) this.#dispatch({ type: 'insertion-committed' });
          }),
          INSERTION_CONTROLLER_TIMEOUT_MS,
        );
        acceptsCommit = false;
        if (result.cancelled === true) this.#dispatch({ type: 'insertion-cancelled' });
        else this.#dispatch({ type: 'inserted', copied: result.copied });
      } catch {
        acceptsCommit = false;
        if (this.#state.phase === 'restoringClipboard') {
          this.#dispatch({ type: 'inserted', copied: false });
        } else if (signal.aborted || this.#state.insertionState === 'cancel-requested') {
          this.#dispatch({ type: 'insertion-cancelled' });
        } else {
          // The production insertion service leaves the requested text on the clipboard when
          // native paste cannot be confirmed. Bound custom/failed ports to the same safe result.
          this.#dispatch({ type: 'inserted', copied: true });
        }
      }
      return;
    }
    if (effect.type === 'teardown') {
      await this.#teardown();
      if (this.#state.phase === 'completed') this.#playSound();
      return;
    }
    this.#scheduleTerminalReset();
  }

  #onFrame(samples: Float32Array, rms: number): void {
    const draining = this.#state.phase === 'transcribing' && this.#captureStopping;
    if (!isCapturePhase(this.#state.phase) && !draining) return;
    const copy = Float32Array.from(samples);
    this.#pcmChunks.push(copy);
    this.#totalSamples += copy.length;
    if (this.#audioStartTimer !== null) {
      clearTimeout(this.#audioStartTimer);
      this.#audioStartTimer = null;
    }
    if (!this.#state.audioReady) this.#dispatch({ type: 'audio-started' });
    const elapsedMs = Math.round((this.#totalSamples / 16_000) * 1_000);
    const now = Date.now();
    if (now - this.#lastLevelAt >= ECHO_LEVEL_EVENT_INTERVAL_MS) {
      this.#lastLevelAt = now;
      this.#dispatch({ type: 'level', rms, elapsedMs });
    }
    if (!draining && this.#silence !== null) {
      const decision = this.#silence.observe({ rms, durationMs: PCM_FRAME_DURATION_MS, elapsedMs });
      if (decision !== null) {
        if (this.#state.phase === 'recordingQuick') {
          this.#dispatch({
            type: 'submit',
            source: decision === 'duration-cap' ? 'duration-cap' : 'silence',
          });
        } else this.#pendingSilenceSubmit = true;
      }
    }
    if (this.#state.phase === 'recordingExtended') {
      if (this.#totalSamples - this.#discardedSamples > MAX_EXTENDED_BUFFERED_SAMPLES) {
        this.#dispatch({
          type: 'fail',
          message: 'Transcription could not keep up with captured audio.',
        });
        return;
      }
      this.#queueExtendedAudio(false);
    }
  }

  async #ensureExtendedStream(): Promise<WhisperStreamingSession> {
    const signal = this.#operationSignal();
    if (this.#stream !== null) return this.#stream;
    if (this.#streamOpening !== null) return raceWithAbort(this.#streamOpening, signal);
    const settings = (this.#sessionSettings ?? this.#settings.get()).transcription;
    const generation = this.#captureGeneration;
    const startup = this.#whisper.startSession(
      {
        modelId: settings.modelId,
        sampleRate: 16_000,
        ...(settings.language === null ? {} : { language: settings.language }),
      },
      signal,
    );
    const opening = startup.then((stream) => {
      if (generation !== this.#captureGeneration || signal.aborted) {
        void stream.cancel().catch(() => undefined);
        throw abortOperationError();
      }
      // Claim the stream before waking abort-bound waiters. If abort wins that race, teardown can
      // still find and cancel this stream; a later non-cooperative startup cancels itself above.
      this.#stream = stream;
      return stream;
    });
    // Teardown may abandon this promise after aborting it. Keep a late worker rejection observed.
    void opening.catch(() => undefined);
    this.#streamOpening = opening;
    try {
      return await raceWithAbort(opening, signal);
    } finally {
      if (this.#streamOpening === opening) this.#streamOpening = null;
    }
  }

  #queueExtendedAudio(force: boolean): void {
    if (this.#streamPushPending) return;
    const available = this.#totalSamples - this.#streamedSamples;
    if (available <= 0 || (!force && available < EXTENDED_PUSH_SAMPLES)) return;
    const start = this.#streamedSamples;
    const count = force ? available : Math.min(available, EXTENDED_PUSH_SAMPLES);
    const relativeStart = start - this.#discardedSamples;
    if (relativeStart < 0) throw new Error('Extended transcription buffer accounting failed');
    const pcm = sliceChunks(this.#pcmChunks, relativeStart, count);
    this.#streamedSamples += count;
    this.#streamPushPending = true;
    const push = this.#streamTail.then(async () => {
      if (this.#streamFailure !== null) {
        throw normalizeOperationError(this.#streamFailure, 'Transcription stream failed');
      }
      const stream = await this.#ensureExtendedStream();
      await stream.push(pcm, this.#abort?.signal);
      if (this.#totalSamples - this.#discardedSamples >= count) {
        this.#pcmChunks = discardChunkPrefix(this.#pcmChunks, count);
        this.#discardedSamples += count;
      }
    });
    // Observe every background rejection immediately, but retain the first failure so submit can
    // never finish a stream after silently dropping audio. Only one push is retained at a time;
    // successful audio is discarded immediately to bound a 30-minute Extended session.
    this.#streamTail = push
      .catch((error: unknown) => {
        this.#streamFailure ??= error;
        if (!isTerminalPhase(this.#state.phase)) {
          this.#dispatch({ type: 'fail', message: publicSessionError(error) });
        }
      })
      .finally(() => {
        this.#streamPushPending = false;
        if (this.#state.phase === 'recordingExtended') this.#queueExtendedAudio(false);
      });
  }

  async #transcribe(): Promise<string> {
    const settings = (this.#sessionSettings ?? this.#settings.get()).transcription;
    if (this.#state.dictationMode === 'extended') {
      await this.#ensureExtendedStream();
      while (this.#streamedSamples < this.#totalSamples || this.#streamPushPending) {
        this.#queueExtendedAudio(true);
        await this.#streamTail;
        if (this.#streamFailure !== null) break;
      }
      if (this.#streamFailure !== null)
        throw normalizeOperationError(this.#streamFailure, 'Stream failed');
      const stream = this.#stream;
      if (stream === null) throw new Error('Transcription stream was unavailable');
      return TranscriptionResultSchema.parse(await stream.finish(this.#abort?.signal)).text;
    }
    if (this.#totalSamples === 0) throw new Error('No audio was captured');
    return TranscriptionResultSchema.parse(
      await this.#whisper.transcribe(
        concatChunks(this.#pcmChunks, this.#totalSamples),
        {
          modelId: settings.modelId,
          sampleRate: 16_000,
          ...(settings.language === null ? {} : { language: settings.language }),
        },
        this.#abort?.signal ?? AbortSignal.abort(),
      ),
    ).text;
  }

  async #stopCapture(generation = this.#captureGeneration): Promise<void> {
    const captureId = this.#captureId;
    this.#captureId = null;
    this.#captureStopping = captureId !== null;
    let stopError: unknown = null;
    try {
      await this.#recording.stopDictation(captureId ?? undefined);
    } catch (error: unknown) {
      stopError = error;
    } finally {
      this.#captureStopping = false;
    }
    await this.#setHelperCaptureDesired(false, generation);
    if (stopError !== null) throw normalizeOperationError(stopError, 'Microphone stop failed');
  }

  async #teardown(): Promise<void> {
    this.#clearRecordingTimers();
    this.#clearResetTimer();
    let captureError: unknown = null;
    try {
      await this.#stopCapture(this.#captureGeneration);
    } catch (error: unknown) {
      captureError = error;
    } finally {
      const opening = this.#streamOpening;
      const stream =
        this.#stream ??
        (opening === null
          ? null
          : await withSoftTimeout(
              opening.catch(() => null),
              STREAM_CANCEL_TIMEOUT_MS,
              null,
            ));
      this.#stream = null;
      this.#streamOpening = null;
      if (stream !== null && this.#state.phase !== 'completed') {
        await withSoftTimeout(
          stream.cancel().catch(() => undefined),
          STREAM_CANCEL_TIMEOUT_MS,
          undefined,
        );
      }
      await withSoftTimeout(
        this.#modelUseOpening.catch(() => undefined),
        MODEL_USE_SETTLE_TIMEOUT_MS,
        undefined,
      );
      this.#modelUse?.release();
      this.#modelUse = null;
      this.#modelUseOpening = Promise.resolve();
      this.#pcmChunks = [];
      this.#totalSamples = 0;
      this.#streamedSamples = 0;
      this.#discardedSamples = 0;
      this.#streamPushPending = false;
      this.#streamFailure = null;
      this.#silence = null;
      if (this.#state.phase !== 'completed') {
        this.#smartSession?.cleanup();
        this.#sessionScreenshotFilename = null;
      }
      this.#teardownComplete = true;
      this.#scheduleTerminalReset();
    }
    if (captureError !== null)
      throw normalizeOperationError(captureError, 'Capture teardown failed');
  }

  #publishActivationTest(): void {
    this.#events.send('activation-test:changed', this.#activationTest);
  }

  #stopActivationTest(): void {
    this.#clearActivationTestHoldTimer();
    if (this.#activationTestExpiryTimer !== null) {
      clearTimeout(this.#activationTestExpiryTimer);
      this.#activationTestExpiryTimer = null;
    }
    this.#activationTestPressedAt = null;
    this.#activationTestOwner = null;
    this.#removeActivationTestOwner?.();
    this.#removeActivationTestOwner = null;
    const wasActive = this.#activationTest.active;
    this.#activationTest = IDLE_ACTIVATION_TEST;
    if (wasActive) {
      void this.#setHelperCaptureDesired(false, this.#captureGeneration);
      this.#publishActivationTest();
    }
  }

  #clearActivationTestHoldTimer(): void {
    if (this.#activationTestHoldTimer !== null) clearTimeout(this.#activationTestHoldTimer);
    this.#activationTestHoldTimer = null;
  }

  #clearResetTimer(): void {
    if (this.#resetTimer !== null) clearTimeout(this.#resetTimer);
    this.#resetTimer = null;
  }

  #publish(): void {
    const snapshot = this.snapshot;
    this.#events.send('echo:session-changed', snapshot);
    for (const listener of this.#listeners) listener(snapshot);
  }

  #recordTerminalOutcome(previous: EchoSessionState, next: EchoSessionState): void {
    if (
      (this.#history === null || !this.#sessionHistoryAllowed) &&
      previous.phase !== next.phase &&
      (next.phase === 'restoringClipboard' || next.phase === 'completed')
    ) {
      this.#smartSession?.cleanup();
      this.#sessionScreenshotFilename = null;
    }
    if (
      this.#history === null ||
      !this.#sessionHistoryAllowed ||
      next.sessionId === null ||
      next.sessionId === this.#historyWrittenSessionId ||
      previous.phase === next.phase
    ) {
      return;
    }
    if (next.dictationMode === null || next.processingMode === null || next.transcript === null) {
      return;
    }
    let outcome: SessionHistoryRecord | null = null;
    if (
      this.#sessionVoiceCommand !== null &&
      (next.phase === 'restoringClipboard' || next.phase === 'completed')
    ) {
      outcome = {
        kind: 'voice-command',
        dictationMode: next.dictationMode,
        processingMode: next.processingMode,
        rawText: next.transcript,
        voiceTrigger: this.#sessionVoiceCommand.trigger,
        voiceSnippet: this.#sessionVoiceCommand.snippet,
      };
    } else if (next.phase === 'restoringClipboard' || next.phase === 'completed') {
      if (next.processingMode === 'raw') {
        outcome = {
          kind: 'raw-completed',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
        };
      } else if (next.fallbackReason !== null) {
        outcome = {
          kind: 'smart-fallback',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
          providerId: this.#sessionProviderId,
          modelId: this.#sessionModelId,
          errorCategory: next.fallbackCategory ?? next.fallbackReason,
        };
      } else if (next.finalText !== null) {
        outcome = {
          kind: 'smart-completed',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
          processedText: next.finalText,
          providerId: this.#sessionProviderId,
          modelId: this.#sessionModelId,
          screenshotFilename: this.#sessionScreenshotFilename,
        };
      }
    } else if (next.phase === 'error') {
      outcome = {
        kind: 'error',
        dictationMode: next.dictationMode,
        processingMode: next.processingMode,
        rawText: next.transcript,
        errorCategory: 'dictation-error',
      };
    }
    if (outcome === null) return;
    this.#historyWrittenSessionId = next.sessionId;
    try {
      const stored = this.#history.record(outcome);
      if (stored && outcome.kind === 'smart-completed') this.#smartSession?.commitScreenshot();
      else this.#smartSession?.cleanup();
    } catch {
      this.#smartSession?.cleanup();
      this.#sessionScreenshotFilename = null;
      // History is optional local persistence. It must never alter an insertion outcome.
    }
  }

  #setHelperCaptureDesired(active: boolean, generation: number): Promise<void> {
    if (generation !== this.#captureGeneration || (this.#disposed && active)) {
      return Promise.resolve();
    }
    this.#helperCaptureDesired = active;
    if (active) this.#captureOffGuaranteed = false;
    this.#helperCaptureRevision += 1;
    if (this.#helperCaptureRunning) return this.#helperCaptureTail;
    this.#helperCaptureRunning = true;
    const reconcile = async () => {
      try {
        while (
          (!this.#disposed || !this.#helperCaptureDesired) &&
          this.#helperCaptureApplied !== this.#helperCaptureDesired
        ) {
          const desired = this.#helperCaptureDesired;
          const revision = this.#helperCaptureRevision;
          try {
            await this.#helper.setSessionCapture(desired);
            if (revision !== this.#helperCaptureRevision) {
              // A later native activation may have re-armed capture while this acknowledgement
              // was in flight. Keep the applied state unknown and reconcile the newer revision.
              this.#helperCaptureApplied = null;
              continue;
            }
            this.#helperCaptureApplied = desired;
            if (!desired) {
              this.#captureOffGuaranteed = true;
              this.#scheduleTerminalReset();
            }
          } catch (error: unknown) {
            this.#helperCaptureApplied = null;
            if (revision !== this.#helperCaptureRevision) continue;
            if (desired) throw normalizeOperationError(error, 'Helper capture could not start');
            try {
              await this.#helper.resetSessionCapture();
              if (revision !== this.#helperCaptureRevision) {
                this.#helperCaptureApplied = null;
                continue;
              }
              this.#helperCaptureApplied = false;
              this.#captureOffGuaranteed = true;
              this.#scheduleTerminalReset();
            } catch (resetError: unknown) {
              this.#captureOffGuaranteed = false;
              throw normalizeOperationError(resetError, 'Helper capture could not be disabled');
            }
          }
        }
      } finally {
        this.#helperCaptureRunning = false;
      }
    };
    this.#helperCaptureTail = this.#helperCaptureTail.then(reconcile, reconcile);
    return this.#helperCaptureTail;
  }

  #scheduleTerminalReset(): void {
    if (
      this.#disposed ||
      !this.#captureOffGuaranteed ||
      !this.#teardownComplete ||
      !isTerminalPhase(this.#state.phase) ||
      this.#resetTimer !== null
    ) {
      return;
    }
    this.#resetTimer = setTimeout(() => {
      this.#resetTimer = null;
      if (
        !this.#captureOffGuaranteed ||
        !this.#teardownComplete ||
        !isTerminalPhase(this.#state.phase)
      ) {
        this.#scheduleTerminalReset();
        return;
      }
      this.#windows.hideWidget();
      this.#dispatch({ type: 'reset' });
    }, ECHO_TERMINAL_DISPLAY_MS);
    this.#resetTimer.unref();
  }

  #captureStillCurrent(signal: AbortSignal): boolean {
    return !signal.aborted && isCapturePhase(this.#state.phase);
  }

  #operationSignal(): AbortSignal {
    return this.#abort?.signal ?? AbortSignal.abort();
  }

  #playSound(): void {
    if (!this.#appPreferences.soundsEnabled) return;
    try {
      this.#sound();
    } catch {
      // Sound cues never affect dictation completion.
    }
  }

  #queueActivationSync(): void {
    if (this.#disposed || this.#helper.readiness.status !== 'ready') return;
    this.#activationSyncRequested = true;
    if (this.#activationSyncScheduled) return;
    this.#activationSyncScheduled = true;
    const operation = async () => {
      try {
        while (
          this.#activationSyncRequested &&
          !this.#disposed &&
          this.#helper.readiness.status === 'ready'
        ) {
          this.#activationSyncRequested = false;
          const settings = this.#settings.get().app;
          await this.#helper.configureActivation(
            settings.enabled && this.#isModelReady(),
            profileBindings(this.#settings.get().dictationProfiles),
          );
        }
      } finally {
        this.#activationSyncScheduled = false;
        if (this.#activationSyncRequested) this.#queueActivationSync();
      }
    };
    this.#activationTail = this.#activationTail.then(operation, operation).catch(() => undefined);
  }

  #replaceCapTimer(milliseconds: number): void {
    if (this.#capTimer !== null) clearTimeout(this.#capTimer);
    this.#capTimer = setTimeout(
      () => this.#dispatch({ type: 'submit', source: 'duration-cap' }),
      Math.max(1, milliseconds),
    );
    this.#capTimer.unref();
  }

  #clearHoldTimer(): void {
    if (this.#holdTimer !== null) clearTimeout(this.#holdTimer);
    this.#holdTimer = null;
  }

  #clearRecordingTimers(): void {
    this.#clearHoldTimer();
    if (this.#capTimer !== null) clearTimeout(this.#capTimer);
    this.#capTimer = null;
    if (this.#audioStartTimer !== null) clearTimeout(this.#audioStartTimer);
    this.#audioStartTimer = null;
  }
}

function profileBindings(profiles: readonly DictationProfile[]) {
  return profiles.map(({ activationKey: key, shift }) => ({ key, shift }));
}

function deepFreezeProfile(profile: DictationProfile): Readonly<DictationProfile> {
  return Object.freeze(structuredClone(profile));
}

function legacySmartSession(
  processor: LegacySmartTranscriptProcessor,
  providerId: string | null,
  modelId: string | null,
): FrozenSmartTranscriptSession {
  return {
    providerId: providerId ?? 'test-provider',
    modelId,
    prepare: () => Promise.resolve(),
    process: async (text, signal) => ({
      text: await processor.process(text, signal),
      screenshotFilename: null,
    }),
    commitScreenshot: () => undefined,
    cleanup: () => undefined,
  };
}

export function discardChunkPrefix(
  chunks: readonly Float32Array[],
  sampleCount: number,
): Float32Array[] {
  if (!Number.isSafeInteger(sampleCount) || sampleCount < 0)
    throw new Error('Discarded sample count is invalid');
  const retained: Float32Array[] = [];
  let remaining = sampleCount;
  for (const chunk of chunks) {
    if (remaining >= chunk.length) {
      remaining -= chunk.length;
      continue;
    }
    if (remaining > 0) {
      retained.push(chunk.slice(remaining));
      remaining = 0;
    } else retained.push(chunk);
  }
  if (remaining !== 0) throw new Error('Cannot discard unavailable PCM samples');
  return retained;
}

function concatChunks(chunks: readonly Float32Array[], total: number): Float32Array {
  return sliceChunks(chunks, 0, total);
}

function sliceChunks(chunks: readonly Float32Array[], start: number, count: number): Float32Array {
  const output = new Float32Array(count);
  let sourceOffset = 0;
  let outputOffset = 0;
  for (const chunk of chunks) {
    const chunkEnd = sourceOffset + chunk.length;
    if (chunkEnd <= start) {
      sourceOffset = chunkEnd;
      continue;
    }
    const from = Math.max(0, start - sourceOffset);
    const available = Math.min(chunk.length - from, count - outputOffset);
    if (available > 0) output.set(chunk.subarray(from, from + available), outputOffset);
    outputOffset += available;
    sourceOffset = chunkEnd;
    if (outputOffset === count) break;
  }
  if (outputOffset !== count) throw new Error('PCM buffer was inconsistent');
  return output;
}

function isCapturePhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'arming' || phase === 'recordingQuick' || phase === 'recordingExtended';
}

function isTerminalPhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'completed' || phase === 'cancelled' || phase === 'error';
}

function raceWithAbort<Value>(operation: Promise<Value>, signal: AbortSignal): Promise<Value> {
  if (signal.aborted) return Promise.reject(abortOperationError());
  return new Promise<Value>((resolve, reject) => {
    const abort = () => reject(abortOperationError());
    signal.addEventListener('abort', abort, { once: true });
    operation.then(
      (value) => {
        signal.removeEventListener('abort', abort);
        resolve(value);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof Error ? error : new Error('Echo operation failed'));
      },
    );
  });
}

function piFallbackCategory(providerId: string, error: unknown): PiFallbackCategory | undefined {
  if (providerId !== 'pi' || !(error instanceof ProviderError)) return undefined;
  switch (error.code) {
    case 'UNAVAILABLE':
      return 'pi-unavailable';
    case 'AUTHENTICATION_FAILED':
      return 'pi-authentication-failed';
    case 'MODEL_NOT_FOUND':
      return 'pi-model-not-found';
    case 'NO_MODELS':
      return 'pi-no-models';
    case 'TIMEOUT':
      return 'pi-timeout';
    case 'INVALID_RESPONSE':
    case 'RESPONSE_TOO_LARGE':
      return 'pi-invalid-response';
    default:
      return 'pi-remote-failure';
  }
}

function normalizeOperationError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}

function abortOperationError(): Error {
  const error = new Error('Echo operation cancelled');
  error.name = 'AbortError';
  return error;
}

function publicSessionError(error: unknown): string {
  void error;
  return 'Dictation could not be completed.';
}

function withSoftTimeout<Value>(
  operation: Promise<Value>,
  timeoutMs: number,
  fallback: Value,
): Promise<Value> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback), timeoutMs);
    timer.unref();
    operation.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}

function withDeadline<Value>(operation: Promise<Value>, timeoutMs: number): Promise<Value> {
  return new Promise<Value>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error('Insertion did not settle in time')),
      timeoutMs,
    );
    timer.unref();
    operation.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error('Insertion failed'));
      },
    );
  });
}
