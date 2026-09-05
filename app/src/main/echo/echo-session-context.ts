import type { ActivationBinding, HelperActivationContext } from '../../shared/helper/protocol';
import { type DictationProfile } from '../../shared/schemas/dictation-profiles';
import { type EchoSessionSnapshot } from '../../shared/schemas/echo-session';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type { Settings } from '../../shared/schemas/settings';
import { type ShortcutCaptureLeaseId } from '../../shared/schemas/shortcut-capture';
import type { WindowManager } from '../app/window-manager';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import { ActivationTestController } from './activation-test-controller';
import { EchoCapturePipeline } from './echo-capture-pipeline';
import { acceptHelperNotification, helperReadinessError } from './echo-session-activation';
import { abortSession, operationSignal } from './echo-session-operation';
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
import {
  playSound,
  publishActivationTest,
  reportOperationalFailure,
  scheduleTerminalReset,
} from './echo-session-presentation';
import { dispatch } from './echo-session-transitions';
import { HelperCaptureReconciler } from './helper-capture-reconciler';
import { ProfileActivationCoordinator } from './profile-activation-coordinator';
import { SessionOutcomeWriter } from './session-outcome-writer';
import { helperCaptureModeForPhase } from './session-phase';
import { IDLE_ECHO_SESSION, type EchoSessionEvent, type EchoSessionState } from './session-reducer';

export interface EchoSessionContextOptions {
  readonly settings: SettingsStore;
  readonly platform: 'win32' | 'darwin';
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
}

/** Per-instance state. The public EchoSessionController owns this context. */
export class EchoSessionContext {
  readonly settings: SettingsStore;
  readonly platform: 'win32' | 'darwin';
  readonly helper: EchoHelperPort;
  readonly captureReconciler: HelperCaptureReconciler;
  readonly capture: EchoCapturePipeline;
  readonly profiles: ProfileActivationCoordinator;
  readonly activationTest: ActivationTestController;
  readonly outcomes: SessionOutcomeWriter;
  readonly insertion: EchoInsertionPort;
  readonly windows: WindowManager;
  readonly events: IpcEventEmitter;
  readonly commands: VoiceCommandMatcherPort | null;
  readonly sound: () => void;
  appPreferences: Settings['app'];
  readonly isModelReady: () => boolean;
  readonly listeners = new Set<(snapshot: EchoSessionSnapshot) => void>();
  readonly removeHelperNotifications: () => void;
  readonly removeHelperReadiness: () => void;
  readonly removeSettings: () => void;
  readonly shortcutCaptureLeases = new Map<
    ShortcutCaptureLeaseId,
    {
      readonly ownerWebContentsId: number;
      removeOnInvalidated: () => void;
      releaseStarted: boolean;
      releaseOperation: Promise<void> | null;
    }
  >();
  state: EchoSessionState = IDLE_ECHO_SESSION;
  abort: AbortController | null = null;
  effectTail: Promise<void> = Promise.resolve();
  resetTimer: ReturnType<typeof setTimeout> | null = null;
  teardownComplete = true;
  teardownInFlight: Promise<void> | null = null;
  sessionSettings: Readonly<Settings> | null = null;
  sessionProfile: Readonly<DictationProfile> | null = null;
  activeActivation: Readonly<ActivationBinding & HelperActivationContext> | null = null;
  pendingOperationalError: string | null = null;
  operationalWidgetGeneration = 0;
  initialized = false;
  shutdownOperation: Promise<void> | null = null;
  smartPreparation: {
    readonly sessionId: string;
    readonly captureGeneration: number;
    readonly session: NonNullable<SessionOutcomeWriter['smartSession']>;
    readonly promise: Promise<void>;
  } | null = null;
  disposed = false;
  // Effects and presentation send events through the controller composition, not each other.
  readonly dispatch = (event: EchoSessionEvent): void => dispatch(this, event);
  constructor(options: EchoSessionContextOptions) {
    this.settings = options.settings;
    this.platform = options.platform;
    this.appPreferences = this.settings.get().app;
    this.helper = options.helper;
    this.insertion = options.insertion;
    this.windows = options.windows;
    this.events = options.events;
    this.commands = options.commands ?? null;
    this.sound = options.sound ?? (() => undefined);
    this.isModelReady = options.isModelReady ?? (() => true);
    const acquireModelUse =
      options.acquireModelUse ??
      (() => Promise.resolve({ status: { state: 'ready' }, release: () => undefined }));
    this.captureReconciler = new HelperCaptureReconciler(this.helper, () =>
      scheduleTerminalReset(this),
    );
    this.profiles = new ProfileActivationCoordinator({
      settings: this.settings,
      helper: this.helper,
      isModelReady: this.isModelReady,
      onSyncFailure: () =>
        reportOperationalFailure(
          this,
          'Keyboard shortcuts could not be enabled. Talking Quill will retry automatically.',
        ),
      onSyncSuccess: () => {
        this.pendingOperationalError = null;
      },
    });
    this.outcomes = new SessionOutcomeWriter({
      history: options.history ?? null,
      smart: options.smartProcessor ?? null,
    });
    this.activationTest = new ActivationTestController({
      publish: (state) => publishActivationTest(this, state),
      requestCaptureOff: () =>
        this.captureReconciler.requestBestEffort('off', this.capture.generation),
      platformUnavailable: this.platform !== 'win32',
    });
    this.capture = new EchoCapturePipeline({
      recording: options.recording,
      whisper: options.whisper,
      captureReconciler: this.captureReconciler,
      windows: this.windows,
      getWidgetSize: () => this.appPreferences.widgetSize,
      playSound: () => playSound(this),
      getState: () => this.state,
      getSignal: () => operationSignal(this),
      abort: () => this.abort?.abort(),
      dispatch: (event) => dispatch(this, event),
      acquireModelUse,
    });
    this.removeHelperNotifications = this.helper.subscribeNotifications((notification) =>
      acceptHelperNotification(this, notification),
    );
    this.removeHelperReadiness = this.helper.subscribeReadiness((readiness) => {
      if (readiness.status === 'ready') {
        if (this.initialized) {
          this.profiles.requestSync();
          this.captureReconciler.requestBestEffort(
            helperCaptureModeForPhase(this.state.phase),
            this.capture.generation,
          );
        }
        return;
      }
      this.captureReconciler.markAppliedUnknown();
      this.captureReconciler.requestBestEffort('off', this.capture.generation);
      if (this.activationTest.state.active) this.activationTest.stop();
      if (
        this.state.audioReady &&
        [
          'arming',
          'recordingQuick',
          'recordingExtended',
          'transcribing',
          'processingSmart',
        ].includes(this.state.phase) &&
        (readiness.status === 'starting' ||
          [
            'unexpected-exit',
            'owner-missing',
            'owner-degraded',
            'request-timeout',
            'handshake-timeout',
            'crash-loop',
            'hook-fault',
            'spawn-failed',
          ].includes(readiness.reason ?? ''))
      ) {
        this.capture.detachNativeCapture();
        dispatch(this, { type: 'keyboard-disconnected' });
        return;
      }
      const message = helperReadinessError(readiness, this.platform);
      if (message !== null) reportOperationalFailure(this, message);
      else if (this.state.phase !== 'idle') abortSession(this, 'target-lost');
    });
    this.removeSettings = this.settings.subscribe((next) => {
      this.appPreferences = next.app;
      this.profiles.requestSync();
      // Privacy grants are frozen at session start. Revocation applies immediately; enabling
      // history while a session is active affects only a future session.
      if (!next.privacy.historyEnabled) this.outcomes.revokeHistory();
    });
  }
}
