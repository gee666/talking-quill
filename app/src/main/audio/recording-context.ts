import type { WebContents } from 'electron';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import type { MicrophonePermissionController } from '../security/microphone-permission';
import type { SystemAudioCaptureController } from '../security/system-audio-capture';
import type {
  MicrophoneDevice,
  MicrophoneDeviceList,
  MicrophoneTestState,
} from '../../shared/schemas/audio';
import type { CaptureWindowClient } from './capture-window-client';

export interface DictationCaptureCallbacks {
  readonly onFrame: (samples: Float32Array, rms: number) => void;
  readonly onUnexpectedStop: (
    reason: 'device-unavailable' | 'system-audio-unavailable' | 'capture-unavailable',
  ) => void;
}

export interface DictationCapture {
  readonly captureId: string;
  readonly activeMicrophoneId: string | null;
  readonly preferredUnavailable: boolean;
}

export interface DictationCaptureOptions {
  readonly includeSystemAudio?: boolean;
}

export interface ActiveDictation extends DictationCapture {
  readonly bindingGeneration: number;
  readonly callbacks: DictationCaptureCallbacks;
}

interface StopInFlight {
  readonly promise: Promise<boolean>;
}

export interface AuthorizedDeviceRefresh {
  readonly webContentsId: number;
  readonly captureId: string;
  readonly operationGeneration: number;
}

export interface DeviceRefreshWaiter {
  readonly generation: number;
  readonly resolve: () => void;
}

/** Internal state shared by recording operations. Only RecordingService owns an instance.
 * Keep generation checks and mutations synchronous across module calls.
 */
export class RecordingContext {
  readonly capture: CaptureWindowClient;
  readonly settings: SettingsStore;
  readonly events: IpcEventEmitter;
  readonly permission: MicrophonePermissionController;
  readonly systemAudio: SystemAudioCaptureController | null;
  removeFrameListener!: () => void;
  removeDeviceListener!: () => void;
  removeDefaultInvalidationListener!: () => void;
  removeStopListener!: () => void;
  captureWebContents: WebContents | null = null;
  captureAttachmentGeneration = 0;
  activeCaptureId: string | null = null;
  activeCaptureKind: 'dictation' | 'test' | null = null;
  activePreferredMicrophoneId: string | null = null;
  activeBindingGeneration = 0;
  activeCaptureActivated = false;
  activeExplicitDeviceAbsent = false;
  activePreferredUnavailable = false;
  pendingDictationGeneration: number | null = null;
  dictation: ActiveDictation | null = null;
  drainingDictation: ActiveDictation | null = null;
  stopInFlight: StopInFlight | null = null;
  ownerWebContents: WebContents | null = null;
  devices: readonly MicrophoneDevice[] = [];
  lastPublishedDeviceSnapshot: MicrophoneDeviceList | null = null;
  state: MicrophoneTestState;
  lastLevelEventAt = 0;
  testObservedRms = 0;
  testSampleCount = 0;
  operation: Promise<void> = Promise.resolve();
  operationGeneration = 0;
  permissionOperation: Promise<void> = Promise.resolve();
  deviceRefreshGeneration = 0;
  lastAuthorizedDeviceRefreshGeneration = 0;
  deviceRefreshDirty = false;
  deviceRefreshInFlight: Promise<void> | null = null;
  deviceRefreshWaiters: DeviceRefreshWaiter[] = [];
  pendingAuthorizedDeviceRefresh: AuthorizedDeviceRefresh | null = null;
  pendingDefaultRebindGeneration: number | null = null;
  defaultRebindAttemptGeneration: number | null = null;
  defaultRebindFollowUp = false;
  defaultRebindInFlight: Promise<void> | null = null;
  disposed = false;
  shutdownPromise: Promise<void> | null = null;
  welcomeMicrophoneBindingKnown = true;
  explicitValidationGeneration = 0;
  onMicrophoneUnavailable: (() => void) | null = null;
  onMicrophoneValidationChanged: ((known: boolean) => void) | null = null;
  onOwnerDestroyed!: () => void;
  onOwnerRenderProcessGone!: () => void;
  onOwnerDidStartNavigation!: (
    _event: Electron.Event,
    _url: string,
    _isInPlace: boolean,
    isMainFrame: boolean,
  ) => void;
  constructor(
    capture: CaptureWindowClient,
    settings: SettingsStore,
    events: IpcEventEmitter,
    permission: MicrophonePermissionController,
    systemAudio: SystemAudioCaptureController | null = null,
  ) {
    this.capture = capture;
    this.settings = settings;
    this.events = events;
    this.permission = permission;
    this.systemAudio = systemAudio;
    const permissionState = permission.getStatus();
    this.state =
      permissionState === 'denied' || permissionState === 'restricted'
        ? {
            status: 'blocked',
            permission: permissionState,
            reason: 'microphone-permission',
          }
        : { status: 'idle', permission: permissionState };
  }
}
