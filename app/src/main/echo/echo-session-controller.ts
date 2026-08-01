import { randomUUID } from 'node:crypto';
import { ECHO_TERMINAL_DISPLAY_MS } from '../../shared/constants/echo-session';
import type { ActivationBinding, HelperNotification } from '../../shared/helper/protocol';
import type { ActivationTestState } from '../../shared/schemas/activation-test';
import type { VoiceCommandMatch } from '../../shared/schemas/commands';
import {
  DEFAULT_GENERAL_PROFILE,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../shared/schemas/dictation-profiles';
import {
  EchoSessionSnapshotSchema,
  type EchoAbortReason,
  type EchoSessionSnapshot,
  type PiFallbackCategory,
} from '../../shared/schemas/echo-session';
import type { HelperReadiness } from '../../shared/schemas/helper-readiness';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
import { deepFreezeShortcut, shortcutsEqual } from '../../shared/schemas/shortcut';
import type { WindowManager } from '../app/window-manager';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import { ProviderError } from '../providers/errors';
import { ActivationTestController } from './activation-test-controller';
import { EchoCapturePipeline } from './echo-capture-pipeline';
import { raceWithAbort } from './echo-operation';
import type {
  EchoHelperPort,
  EchoHistoryPort,
  EchoInsertionPort,
  EchoModelUseGrant,
  EchoRecordingPort,
  EchoWhisperPort,
  SmartTranscriptProcessor,
  VoiceCommandMatcherPort,
} from './echo-session-ports';
import { HelperCaptureReconciler } from './helper-capture-reconciler';
import { ProfileActivationCoordinator } from './profile-activation-coordinator';
import {
  IDLE_ECHO_SESSION,
  reduceEchoSession,
  type EchoSessionEffect,
  type EchoSessionEvent,
  type EchoSessionState,
} from './session-reducer';
import { SessionOutcomeWriter } from './session-outcome-writer';
import { helperCaptureModeForPhase, isTerminalPhase } from './session-phase';

export { discardChunkPrefix } from './pcm-buffer';
export type {
  EchoHelperPort,
  EchoHistoryPort,
  EchoInsertionPort,
  EchoModelUseGrant,
  EchoRecordingPort,
  EchoWhisperPort,
  LegacySmartTranscriptProcessor,
  SmartTranscriptProcessor,
  VoiceCommandMatcherPort,
} from './echo-session-ports';

const INSERTION_CONTROLLER_TIMEOUT_MS = 5_000;

export class EchoSessionController {
  readonly #settings: SettingsStore;
  readonly #helper: EchoHelperPort;
  readonly #captureReconciler: HelperCaptureReconciler;
  readonly #capture: EchoCapturePipeline;
  readonly #profiles: ProfileActivationCoordinator;
  readonly #activationTest: ActivationTestController;
  readonly #outcomes: SessionOutcomeWriter;
  readonly #insertion: EchoInsertionPort;
  readonly #windows: WindowManager;
  readonly #events: IpcEventEmitter;
  readonly #commands: VoiceCommandMatcherPort | null;
  readonly #sound: () => void;
  #appPreferences: Settings['app'];
  readonly #isModelReady: () => boolean;
  readonly #listeners = new Set<(snapshot: EchoSessionSnapshot) => void>();
  readonly #removeHelperNotifications: () => void;
  readonly #removeHelperReadiness: () => void;
  readonly #removeSettings: () => void;
  readonly #shortcutCaptureOwners = new Map<number, () => void>();
  #state: EchoSessionState = IDLE_ECHO_SESSION;
  #abort: AbortController | null = null;
  #effectTail: Promise<void> = Promise.resolve();
  #resetTimer: ReturnType<typeof setTimeout> | null = null;
  #teardownComplete = true;
  #teardownInFlight: Promise<void> | null = null;
  #sessionSettings: Readonly<Settings> | null = null;
  #sessionProfile: Readonly<DictationProfile> | null = null;
  #activeBinding: Readonly<ActivationBinding> | null = null;
  #pendingOperationalError: string | null = null;
  #shutdownOperation: Promise<void> | null = null;
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
    this.#helper = options.helper;
    this.#insertion = options.insertion;
    this.#windows = options.windows;
    this.#events = options.events;
    this.#commands = options.commands ?? null;
    this.#sound = options.sound ?? (() => undefined);
    this.#isModelReady = options.isModelReady ?? (() => true);
    const acquireModelUse =
      options.acquireModelUse ??
      (() => Promise.resolve({ status: { state: 'ready' }, release: () => undefined }));
    this.#captureReconciler = new HelperCaptureReconciler(this.#helper, () =>
      this.#scheduleTerminalReset(),
    );
    this.#profiles = new ProfileActivationCoordinator({
      settings: this.#settings,
      helper: this.#helper,
      isModelReady: this.#isModelReady,
      onSyncFailure: () =>
        this.#reportOperationalFailure(
          'Keyboard shortcuts could not be enabled. Restart Talking Quill or reinstall the app.',
        ),
    });
    this.#outcomes = new SessionOutcomeWriter({
      history: options.history ?? null,
      smart: options.smartProcessor ?? null,
    });
    this.#activationTest = new ActivationTestController({
      publish: (state) => this.#publishActivationTest(state),
      requestCaptureOff: () =>
        this.#captureReconciler.requestBestEffort('off', this.#capture.generation),
    });
    this.#capture = new EchoCapturePipeline({
      recording: options.recording,
      whisper: options.whisper,
      captureReconciler: this.#captureReconciler,
      windows: this.#windows,
      getWidgetSize: () => this.#appPreferences.widgetSize,
      playSound: () => this.#playSound(),
      getState: () => this.#state,
      getSignal: () => this.#operationSignal(),
      abort: () => this.#abort?.abort(),
      dispatch: (event) => this.#dispatch(event),
      acquireModelUse,
    });
    this.#removeHelperNotifications = this.#helper.subscribeNotifications((notification) =>
      this.acceptHelperNotification(notification),
    );
    this.#removeHelperReadiness = this.#helper.subscribeReadiness((readiness) => {
      if (readiness.status === 'ready') {
        this.#profiles.requestSync();
        this.#captureReconciler.requestBestEffort(
          helperCaptureModeForPhase(this.#state.phase),
          this.#capture.generation,
        );
      } else {
        this.#captureReconciler.markAppliedUnknown();
        this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
        if (this.#activationTest.state.active) this.#activationTest.stop();
        const message = helperReadinessError(readiness);
        if (message !== null) this.#reportOperationalFailure(message);
        else if (this.#state.phase !== 'idle') this.abort('target-lost');
      }
    });
    this.#removeSettings = this.#settings.subscribe((next) => {
      this.#appPreferences = next.app;
      this.#profiles.requestSync();
      // Privacy grants are frozen at session start. Revocation applies immediately; enabling
      // history while a session is active affects only a future session.
      if (!next.privacy.historyEnabled) this.#outcomes.revokeHistory();
    });
  }

  get activationTestState(): ActivationTestState {
    return this.#activationTest.state;
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
    this.#profiles.requestSync();
    const message = helperReadinessError(this.#helper.readiness);
    if (message !== null) this.#reportOperationalFailure(message);
    else this.#publish();
  }

  startActivationTest(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
  ): ActivationTestState {
    const unavailableReason = !this.#settings.get().app.enabled
      ? 'app-disabled'
      : this.#helper.readiness.status !== 'ready'
        ? 'helper-unavailable'
        : this.#state.phase !== 'idle'
          ? 'session-active'
          : null;
    return this.#activationTest.start(ownerWebContentsId, onDestroyed, unavailableReason);
  }

  stopActivationTest(ownerWebContentsId?: number): ActivationTestState {
    return this.#activationTest.stop(ownerWebContentsId);
  }

  async startShortcutCapture(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
  ): Promise<void> {
    if (this.#state.phase !== 'idle' || this.#activationTest.state.active) {
      throw new Error('Shortcut capture is unavailable during an active session or shortcut test');
    }
    if (this.#shortcutCaptureOwners.has(ownerWebContentsId)) return;
    const removeOnInvalidated = onDestroyed(() => {
      void this.#releaseShortcutCaptureOwner(ownerWebContentsId).catch(() => undefined);
    });
    this.#shortcutCaptureOwners.set(ownerWebContentsId, removeOnInvalidated);
    await this.#profiles.beginShortcutCapture(ownerWebContentsId);
  }

  async stopShortcutCapture(ownerWebContentsId: number): Promise<void> {
    await this.#releaseShortcutCaptureOwner(ownerWebContentsId);
  }

  async #releaseShortcutCaptureOwner(ownerWebContentsId: number): Promise<void> {
    const removeOnInvalidated = this.#shortcutCaptureOwners.get(ownerWebContentsId);
    if (removeOnInvalidated !== undefined) {
      this.#shortcutCaptureOwners.delete(ownerWebContentsId);
      removeOnInvalidated();
    }
    await this.#profiles.endShortcutCapture(ownerWebContentsId);
  }

  acceptHelperNotification(notification: HelperNotification): void {
    try {
      this.#onHelperNotification(notification);
    } catch {
      // Treat malformed or out-of-contract native notifications as potentially capture-armed.
      // This preserves fail-open cleanup across mixed helper/app versions.
      this.#captureReconciler.markNativeCaptureArmed();
      this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
      this.#reportOperationalFailure('Dictation could not start. Please try again.');
    }
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
    this.#profiles.requestSync();
  }

  updateGeneral(patch: PublicSettingsPatch): Promise<Settings> {
    const nextEnabled = patch.app?.enabled ?? this.#settings.get().app.enabled;
    if (!nextEnabled) {
      if (this.#activationTest.state.active) this.#activationTest.stop();
      if (this.#state.phase !== 'idle') this.cancel();
    }
    return this.#profiles.updateGeneral(patch);
  }

  createProfile(input: DictationProfileCreate): Promise<Settings> {
    return this.#profiles.createProfile(input);
  }

  updateProfile(id: string, patch: DictationProfilePatch): Promise<Settings> {
    return this.#profiles.updateProfile(id, patch);
  }

  deleteProfile(id: string): Promise<Settings> {
    return this.#profiles.deleteProfile(id);
  }

  resetProfile(id: string): Promise<Settings> {
    return this.#profiles.resetProfile(id);
  }

  shutdown(): Promise<void> {
    if (this.#shutdownOperation !== null) return this.#shutdownOperation;
    let resolveShutdown!: () => void;
    let rejectShutdown!: (error: unknown) => void;
    const operation = new Promise<void>((resolve, reject) => {
      resolveShutdown = resolve;
      rejectShutdown = reject;
    });
    this.#shutdownOperation = operation;
    this.#disposed = true;
    this.#profiles.dispose();
    this.#captureReconciler.beginShutdown();
    this.#abort?.abort();
    this.#activationTest.stop();
    for (const removeOnDestroyed of this.#shortcutCaptureOwners.values()) removeOnDestroyed();
    this.#shortcutCaptureOwners.clear();
    this.#clearResetTimer();
    this.#dispatch({ type: 'abort', reason: 'shutdown' });
    void (async () => {
      try {
        await this.#drainEffects();
        await this.#teardown().catch(() => undefined);
      } finally {
        this.#clearResetTimer();
        this.#removeSettings();
        this.#removeHelperReadiness();
        this.#removeHelperNotifications();
        this.#listeners.clear();
      }
    })().then(resolveShutdown, rejectShutdown);
    return operation;
  }

  #onHelperNotification(notification: HelperNotification): void {
    if (this.#disposed || notification.method === 'paste.committed') return;
    if (notification.method === 'activation.event') {
      const startsActivation = notification.params.phase !== 'up';
      if (startsActivation) {
        // Older helper versions arm session-key capture before publishing activation. Keep this
        // compatibility repair until every supported installed helper follows the new contract.
        this.#captureReconciler.markNativeCaptureArmed();
      }
      if (this.#profiles.shortcutCaptureActive) {
        if (startsActivation) {
          this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
        }
        return;
      }
      if (this.#activationTest.state.active) {
        this.#activationTest.accept(notification, this.#settings.get().dictationProfiles);
        if (startsActivation) {
          this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
        }
        return;
      }
      if (startsActivation) {
        if (this.#state.phase === 'idle') {
          const settings = this.#settings.get();
          if (
            !settings.app.enabled ||
            !this.#isModelReady() ||
            this.#helper.readiness.status !== 'ready'
          ) {
            this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
            return;
          }
          const profile = settings.dictationProfiles.find(
            (candidate) =>
              candidate.id === notification.params.profileId &&
              shortcutsEqual(candidate.shortcut, notification.params.shortcut),
          );
          if (profile === undefined) {
            this.#captureReconciler.requestBestEffort('off', this.#capture.generation);
            return;
          }
          this.#sessionSettings = settings;
          this.#sessionProfile = deepFreezeProfile(profile);
          const now = Date.now();
          this.#activeBinding =
            notification.params.phase === 'complete'
              ? null
              : freezeActivationBinding(notification.params);
          this.#dispatch({
            type: 'shortcut-down',
            sessionId: randomUUID(),
            alternate: profile.shortcut.modifiers.shift,
            processingMode: profile.processingMode,
            now: notification.params.phase === 'complete' ? now - notification.params.heldMs : now,
          });
          if (notification.params.phase === 'complete') {
            this.#dispatch({ type: 'shortcut-up', now });
          }
        } else if (
          this.#state.phase === 'recordingQuick' ||
          this.#state.phase === 'recordingExtended'
        ) {
          if (
            this.#sessionProfile !== null &&
            this.#sessionProfile.id === notification.params.profileId
          ) {
            this.#activeBinding =
              notification.params.phase === 'complete'
                ? null
                : freezeActivationBinding(notification.params);
            this.#dispatch({ type: 'submit', source: 'shortcut' });
          } else {
            this.#captureReconciler.requestBestEffort(
              helperCaptureModeForPhase(this.#state.phase),
              this.#capture.generation,
            );
          }
        } else {
          // Active phases which do not own this shortcut explicitly restore their capture state.
          this.#captureReconciler.requestBestEffort(
            helperCaptureModeForPhase(this.#state.phase),
            this.#capture.generation,
          );
        }
      } else {
        if (
          this.#activeBinding === null ||
          !activationBindingsEqual(this.#activeBinding, notification.params)
        ) {
          return;
        }
        this.#activeBinding = null;
        this.#dispatch({ type: 'shortcut-up', now: Date.now() });
      }
      return;
    }
    if (notification.params.phase !== 'down') return;
    if (notification.params.key === 'escape') this.cancel();
    else this.#dispatch({ type: 'submit', source: 'enter' });
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
    if (transition.state.phase === 'transcribing' && this.#captureIsMissing()) {
      this.#abort?.abort();
      this.#abort = new AbortController();
    }
    this.#state = transition.state;
    this.#manageSessionTransition(previous, transition.state);
    this.#outcomes.observeTransition(previous, transition.state);
    // Effect ownership is established before notifying observers, so a broken subscriber cannot
    // strand the state machine after its state has already advanced.
    for (const effect of transition.effects) this.#enqueueEffect(effect);
    this.#publish();
    if (
      transition.state.phase === 'error' &&
      transition.state.sessionId === null &&
      previous.phase === 'idle'
    ) {
      this.#scheduleTerminalReset();
    }
    if (event.type === 'reset' && this.#pendingOperationalError !== null) {
      const message = this.#pendingOperationalError;
      this.#pendingOperationalError = null;
      this.#reportOperationalFailure(message);
    }
  }

  #manageSessionTransition(previous: EchoSessionState, next: EchoSessionState): void {
    if (next.phase === 'idle' || isTerminalPhase(next.phase)) this.#activeBinding = null;
    if (previous.phase === 'idle' && next.phase === 'arming') {
      this.#capture.beginGeneration();
      this.#abort = new AbortController();
      this.#teardownComplete = false;
      const sessionSettings = this.#sessionSettings ?? this.#settings.get();
      this.#sessionSettings = sessionSettings;
      const processingMode = this.#state.processingMode;
      if (processingMode === null) throw new Error('Session processing mode was unavailable');
      this.#outcomes.beginSession(
        sessionSettings,
        this.#sessionProfile ?? DEFAULT_GENERAL_PROFILE,
        processingMode,
      );
      this.#capture.arm(sessionSettings);
    }
    const previousHelperMode = helperCaptureModeForPhase(previous.phase);
    const nextHelperMode = helperCaptureModeForPhase(next.phase);
    if (
      previousHelperMode !== nextHelperMode &&
      !(previous.phase === 'idle' && next.phase === 'arming')
    ) {
      this.#captureReconciler.requestBestEffort(nextHelperMode, this.#capture.generation);
    }
    this.#capture.observeTransition(previous, next);
  }

  async #drainEffects(): Promise<void> {
    let drained: Promise<void>;
    do {
      drained = this.#effectTail;
      await drained.catch(() => undefined);
    } while (drained !== this.#effectTail);
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
      await this.#capture.startCapture();
      return;
    }
    if (effect.type === 'begin-extended-transcription') {
      await this.#capture.beginExtendedTranscription();
      return;
    }
    if (effect.type === 'stop-and-transcribe') {
      await this.#capture.stopCapture();
      const signal = this.#operationSignal();
      const smartSession = this.#outcomes.smartSession;
      const smartPreparation =
        this.#state.processingMode === 'smart' && smartSession !== null
          ? raceWithAbort(smartSession.prepare(signal), signal)
          : Promise.resolve();
      // Screenshot/provider preparation and local inference are independent. Run them together so
      // Smart mode pays only the slower latency rather than adding both waits after recording.
      const [, text] = await Promise.all([
        smartPreparation,
        raceWithAbort(this.#capture.transcribe(), signal),
      ]);
      const match: VoiceCommandMatch | null = this.#commands?.match(text) ?? null;
      // In Smart mode, only an exact local match bypasses the monitor. Fuzzy and cross-language
      // candidates must be reviewed by Smart processing before they can execute.
      const executeImmediately =
        match !== null && (this.#state.processingMode !== 'smart' || match.kind === 'exact');
      if (executeImmediately) {
        this.#outcomes.discardSmartSession();
        this.#outcomes.setVoiceCommand(match.command);
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
      const smartSession = this.#outcomes.smartSession;
      if (smartSession === null) {
        this.#dispatch({ type: 'abort', reason: 'provider-error' });
        return;
      }
      const signal = this.#operationSignal();
      try {
        const result = await raceWithAbort(smartSession.process(effect.text, signal), signal);
        if (result.voiceCommand !== undefined && result.voiceCommand !== null) {
          this.#outcomes.discardSmartSession();
          this.#outcomes.setVoiceCommand(result.voiceCommand);
          this.#dispatch({
            type: 'voice-command-matched',
            transcript: effect.text,
            command: result.voiceCommand,
          });
        } else {
          this.#outcomes.setScreenshotFilename(result.screenshotFilename);
          this.#dispatch({ type: 'smart-completed', text: result.text });
        }
      } catch (error: unknown) {
        const providerId = smartSession.providerId;
        this.#outcomes.discardSmartSession();
        if (!signal.aborted && this.#state.phase === 'processingSmart') {
          const reason =
            error instanceof ProviderError && error.code === 'TIMEOUT'
              ? 'timeout'
              : 'provider-error';
          this.abort(reason, piFallbackCategory(providerId, error));
        }
      }
      return;
    }
    if (effect.type === 'insert') {
      const signal = this.#operationSignal();
      const insertionAbort = new AbortController();
      const abortInsertion = () => insertionAbort.abort(signal.reason);
      if (signal.aborted) abortInsertion();
      else signal.addEventListener('abort', abortInsertion, { once: true });
      let acceptsCommit = true;
      try {
        const result = await withDeadline(
          this.#insertion.insert(effect.text, insertionAbort.signal, () => {
            if (acceptsCommit) this.#dispatch({ type: 'insertion-committed' });
          }),
          INSERTION_CONTROLLER_TIMEOUT_MS,
          () => insertionAbort.abort(new Error('Insertion controller deadline exceeded')),
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
      } finally {
        signal.removeEventListener('abort', abortInsertion);
      }
      return;
    }
    await this.#teardown();
    if (this.#state.phase === 'completed') this.#playSound();
  }

  #teardown(): Promise<void> {
    if (this.#teardownInFlight !== null) return this.#teardownInFlight;
    if (this.#teardownComplete && this.#captureReconciler.captureOffGuaranteed) {
      return Promise.resolve();
    }
    const teardown = this.#performTeardown();
    this.#teardownInFlight = teardown;
    const clear = () => {
      if (this.#teardownInFlight === teardown) this.#teardownInFlight = null;
    };
    void teardown.then(clear, clear);
    return teardown;
  }

  async #performTeardown(): Promise<void> {
    try {
      await this.#capture.performTeardown(() => this.#clearResetTimer());
    } finally {
      if (this.#state.phase !== 'completed') this.#outcomes.discardSmartSession();
      this.#teardownComplete = true;
      this.#scheduleTerminalReset();
    }
  }

  #publishActivationTest(state: ActivationTestState): void {
    try {
      this.#events.send('activation-test:changed', state);
    } catch {
      // Renderer publication is ancillary to activation-test ownership and cleanup.
    }
  }

  #clearResetTimer(): void {
    if (this.#resetTimer !== null) clearTimeout(this.#resetTimer);
    this.#resetTimer = null;
  }

  #publish(): void {
    const snapshot = this.snapshot;
    try {
      this.#events.send('echo:session-changed', snapshot);
    } catch {
      // A renderer disappearing cannot interrupt state-machine effects.
    }
    for (const listener of this.#listeners) {
      try {
        listener(snapshot);
      } catch {
        // Subscribers are independent observers and cannot own controller progress.
      }
    }
  }

  #scheduleTerminalReset(): void {
    if (
      this.#disposed ||
      !this.#captureReconciler.captureOffGuaranteed ||
      !this.#teardownComplete ||
      !isTerminalPhase(this.#state.phase) ||
      this.#resetTimer !== null
    ) {
      return;
    }
    this.#resetTimer = setTimeout(() => {
      this.#resetTimer = null;
      if (
        !this.#captureReconciler.captureOffGuaranteed ||
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

  #captureIsMissing(): boolean {
    return this.#capture.captureId === null;
  }

  #operationSignal(): AbortSignal {
    return this.#abort?.signal ?? AbortSignal.abort();
  }

  #reportOperationalFailure(message: string): void {
    if (this.#disposed) return;
    if (
      this.#state.phase === 'inserting' ||
      this.#state.phase === 'restoringClipboard' ||
      isTerminalPhase(this.#state.phase)
    ) {
      // Never overwrite an insertion that may already have committed. Show this independent
      // operational failure after the current outcome has finished its truthful display.
      this.#pendingOperationalError = message;
      return;
    }
    if (this.#state.phase === 'idle') {
      this.#dispatch({ type: 'operational-failure', message });
    } else {
      this.#abort?.abort();
      this.#dispatch({ type: 'fail', message });
    }
    try {
      if (!this.#windows.showWidget(this.#appPreferences.widgetSize, null)) {
        this.#windows.showMain();
      }
    } catch {
      // If the widget renderer itself is unavailable, keep the main window as the visible fallback.
      this.#windows.showMain();
    }
  }

  #playSound(): void {
    if (!this.#appPreferences.soundsEnabled) return;
    try {
      this.#sound();
    } catch {
      // Sound cues never affect dictation completion.
    }
  }
}

function deepFreezeProfile(profile: DictationProfile): Readonly<DictationProfile> {
  const clone = structuredClone(profile);
  clone.shortcut = deepFreezeShortcut(clone.shortcut);
  return Object.freeze(clone);
}

function freezeActivationBinding(binding: ActivationBinding): Readonly<ActivationBinding> {
  return Object.freeze({
    profileId: binding.profileId,
    shortcut: deepFreezeShortcut(binding.shortcut),
  });
}

function activationBindingsEqual(
  left: Readonly<ActivationBinding>,
  right: ActivationBinding,
): boolean {
  return left.profileId === right.profileId && shortcutsEqual(left.shortcut, right.shortcut);
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

function publicSessionError(error: unknown): string {
  void error;
  return 'Dictation could not be completed.';
}

function helperReadinessError(readiness: HelperReadiness): string | null {
  if (readiness.status === 'permission-required') {
    return 'Keyboard shortcuts need system permission. Open Talking Quill Settings to fix it.';
  }
  if (readiness.status === 'unavailable' || readiness.status === 'incompatible') {
    return 'Keyboard shortcuts are unavailable. Restart Talking Quill or reinstall the app.';
  }
  return null;
}

function withDeadline<Value>(
  operation: Promise<Value>,
  timeoutMs: number,
  onTimeout: () => void,
): Promise<Value> {
  return new Promise<Value>((resolve, reject) => {
    const timer = setTimeout(() => {
      onTimeout();
      reject(new Error('Insertion did not settle in time'));
    }, timeoutMs);
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
