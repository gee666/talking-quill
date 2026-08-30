import type { WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import {
  CAPTURE_CANCEL_TIMEOUT_MS,
  DEFAULT_MICROPHONE_REBIND_ATTEMPTS,
} from '../../shared/constants/audio';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import type { MicrophonePermissionController } from '../security/microphone-permission';
import type { SystemAudioCaptureController } from '../security/system-audio-capture';
import type {
  MicrophoneDevice,
  MicrophoneDeviceList,
  MicrophoneTestState,
} from '../../shared/schemas/audio';
import { CaptureClientError } from './capture-window-client';
import type { CaptureWindowClient } from './capture-window-client';

const LEVEL_EVENT_INTERVAL_MS = 50;

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

interface ActiveDictation extends DictationCapture {
  readonly bindingGeneration: number;
  readonly callbacks: DictationCaptureCallbacks;
}

interface StopInFlight {
  readonly promise: Promise<boolean>;
}

interface AuthorizedDeviceRefresh {
  readonly webContentsId: number;
  readonly captureId: string;
  readonly operationGeneration: number;
}

interface DeviceRefreshWaiter {
  readonly generation: number;
  readonly resolve: () => void;
}

export class RecordingService {
  readonly #capture: CaptureWindowClient;
  readonly #settings: SettingsStore;
  readonly #events: IpcEventEmitter;
  readonly #permission: MicrophonePermissionController;
  readonly #systemAudio: SystemAudioCaptureController | null;
  readonly #removeFrameListener: () => void;
  readonly #removeDeviceListener: () => void;
  readonly #removeDefaultInvalidationListener: () => void;
  readonly #removeStopListener: () => void;
  #captureWebContents: WebContents | null = null;
  #captureAttachmentGeneration = 0;
  #activeCaptureId: string | null = null;
  #activeCaptureKind: 'dictation' | 'test' | null = null;
  #activePreferredMicrophoneId: string | null = null;
  #activeBindingGeneration = 0;
  #activeCaptureActivated = false;
  #activeExplicitDeviceAbsent = false;
  #activePreferredUnavailable = false;
  #pendingDictationGeneration: number | null = null;
  #dictation: ActiveDictation | null = null;
  #drainingDictation: ActiveDictation | null = null;
  #stopInFlight: StopInFlight | null = null;
  #ownerWebContents: WebContents | null = null;
  #devices: readonly MicrophoneDevice[] = [];
  #lastPublishedDeviceSnapshot: MicrophoneDeviceList | null = null;
  #state: MicrophoneTestState;
  #lastLevelEventAt = 0;
  #testObservedRms = 0;
  #testSampleCount = 0;
  #operation: Promise<void> = Promise.resolve();
  #operationGeneration = 0;
  #permissionOperation: Promise<void> = Promise.resolve();
  #deviceRefreshGeneration = 0;
  #lastAuthorizedDeviceRefreshGeneration = 0;
  #deviceRefreshDirty = false;
  #deviceRefreshInFlight: Promise<void> | null = null;
  #deviceRefreshWaiters: DeviceRefreshWaiter[] = [];
  #pendingAuthorizedDeviceRefresh: AuthorizedDeviceRefresh | null = null;
  #pendingDefaultRebindGeneration: number | null = null;
  #defaultRebindAttemptGeneration: number | null = null;
  #defaultRebindFollowUp = false;
  #defaultRebindInFlight: Promise<void> | null = null;
  #disposed = false;
  #shutdownPromise: Promise<void> | null = null;
  #welcomeMicrophoneBindingKnown = true;
  #explicitValidationGeneration = 0;
  #onMicrophoneUnavailable: (() => void) | null = null;
  #onMicrophoneValidationChanged: ((known: boolean) => void) | null = null;

  constructor(
    capture: CaptureWindowClient,
    settings: SettingsStore,
    events: IpcEventEmitter,
    permission: MicrophonePermissionController,
    systemAudio: SystemAudioCaptureController | null = null,
  ) {
    this.#capture = capture;
    this.#settings = settings;
    this.#events = events;
    this.#permission = permission;
    this.#systemAudio = systemAudio;
    const permissionState = permission.getStatus();
    this.#state =
      permissionState === 'denied' || permissionState === 'restricted'
        ? {
            status: 'blocked',
            permission: permissionState,
            reason: 'microphone-permission',
          }
        : { status: 'idle', permission: permissionState };
    this.#removeFrameListener = capture.onFrame((frame) => {
      const dictation =
        this.#dictation?.captureId === frame.captureId
          ? this.#dictation
          : this.#drainingDictation?.captureId === frame.captureId
            ? this.#drainingDictation
            : null;
      if (dictation !== null) {
        dictation.callbacks.onFrame(frame.samples, frame.rms);
        return;
      }
      if (frame.captureId !== this.#activeCaptureId || this.#state.status !== 'active') return;
      this.#testObservedRms = Math.max(this.#testObservedRms, frame.rms);
      this.#testSampleCount += frame.samples.length;
      const now = Date.now();
      if (now - this.#lastLevelEventAt < LEVEL_EVENT_INTERVAL_MS) return;
      this.#lastLevelEventAt = now;
      this.#events.send('recording:test-level', {
        captureId: frame.captureId,
        rms: frame.rms,
      });
    });
    this.#removeDeviceListener = capture.onDevicesChanged((defaultInvalidated) => {
      if (this.#disposed) return;
      if (this.#settings.get().recording.preferredMicrophoneId !== null) {
        this.#validateExplicitBindingAfterInvalidation();
        return;
      }
      this.#refreshAfterInputInvalidation();
      if (!defaultInvalidated) this.#invalidateDefaultEvidence();
    });
    this.#removeDefaultInvalidationListener = capture.onDefaultInvalidated(
      (captureId, bindingGeneration) => {
        if (
          captureId !== this.#activeCaptureId ||
          bindingGeneration < this.#activeBindingGeneration ||
          bindingGeneration > this.#activeBindingGeneration + 1
        ) {
          return;
        }
        if (this.#activePreferredMicrophoneId !== null) {
          if (!this.#activePreferredUnavailable) return;
          this.#validateExplicitBindingAfterInvalidation();
          this.#queueDefaultRebind(bindingGeneration);
          return;
        }
        this.#refreshAfterInputInvalidation();
        this.#invalidateDefaultEvidence();
        this.#queueDefaultRebind(bindingGeneration);
      },
    );
    this.#removeStopListener = capture.onUnexpectedStop((captureId, reason) => {
      if (captureId !== this.#activeCaptureId) return;
      const dictation = this.#dictation;
      this.#activeCaptureId = null;
      this.#activeCaptureKind = null;
      this.#activePreferredMicrophoneId = null;
      this.#activeCaptureActivated = false;
      this.#activeExplicitDeviceAbsent = false;
      this.#activePreferredUnavailable = false;
      this.#pendingDefaultRebindGeneration = null;
      this.#defaultRebindAttemptGeneration = null;
      this.#defaultRebindFollowUp = false;
      this.#dictation = null;
      this.#clearOwner();
      this.#permission.release(captureId);
      this.#systemAudio?.release(captureId);
      if (reason === 'device-unavailable') this.#notifyMicrophoneUnavailable();
      if (dictation !== null) {
        try {
          dictation.callbacks.onUnexpectedStop(reason);
        } catch {
          // Local capture ownership is already released; consumer failure cannot undo cleanup.
        }
        return;
      }
      this.#setState({
        status: 'unavailable',
        permission: this.#permission.getStatus(),
        reason: reason === 'system-audio-unavailable' ? 'capture-unavailable' : reason,
      });
    });
  }

  setWelcomeEvidenceInvalidator(listener: () => void): void {
    this.#onMicrophoneUnavailable = listener;
  }

  setWelcomeEvidenceValidationListener(listener: (known: boolean) => void): void {
    this.#onMicrophoneValidationChanged = listener;
  }

  microphoneReadyForWelcome(): boolean {
    return this.#permission.getStatus() === 'granted' && this.#welcomeMicrophoneBindingKnown;
  }

  attachCapture(webContents: WebContents): void {
    this.#captureAttachmentGeneration += 1;
    this.#pendingAuthorizedDeviceRefresh = null;
    this.#deviceRefreshDirty = false;
    this.#deviceRefreshGeneration += 1;
    for (const waiter of this.#deviceRefreshWaiters) waiter.resolve();
    this.#deviceRefreshWaiters = [];
    this.#captureWebContents = webContents;
    this.#capture.attach(webContents);
  }

  /** Entry point for renderer devicechange and the future native default-device hook. */
  invalidateInputDevices(): void {
    if (this.#disposed) return;
    if (this.#settings.get().recording.preferredMicrophoneId !== null) {
      this.#validateExplicitBindingAfterInvalidation();
      if (this.#activeCaptureId !== null && this.#activePreferredUnavailable) {
        this.#queueDefaultRebind(this.#activeBindingGeneration);
      }
      return;
    }
    this.#refreshAfterInputInvalidation();
    this.#invalidateDefaultEvidence();
    if (this.#activeCaptureId !== null) {
      this.#queueDefaultRebind(this.#activeBindingGeneration);
    }
  }

  #refreshAfterInputInvalidation(): Readonly<{
    generation: number;
    promise: Promise<void>;
  }> {
    const captureId = this.#activeCaptureId;
    const captureWebContents = this.#captureWebContents;
    const canAuthorize =
      captureId !== null &&
      captureWebContents !== null &&
      !captureWebContents.isDestroyed() &&
      this.#activeCaptureActivated;
    const promise = this.#refreshDevices(
      canAuthorize
        ? {
            webContentsId: captureWebContents.id,
            captureId,
            operationGeneration: this.#operationGeneration,
          }
        : undefined,
    );
    return { generation: this.#deviceRefreshGeneration, promise };
  }

  #validateExplicitBindingAfterInvalidation(): void {
    const preferred = this.#settings.get().recording.preferredMicrophoneId;
    if (preferred === null) return;
    const validationGeneration = ++this.#explicitValidationGeneration;
    this.#setWelcomeMicrophoneBindingKnown(false);
    const refresh = this.#refreshAfterInputInvalidation();
    void refresh.promise.then(() => {
      if (
        this.#disposed ||
        validationGeneration !== this.#explicitValidationGeneration ||
        this.#settings.get().recording.preferredMicrophoneId !== preferred
      ) {
        return;
      }
      const authoritative = this.#lastAuthorizedDeviceRefreshGeneration >= refresh.generation;
      if (authoritative) {
        if (
          this.#devices.some((device) => device.deviceId === preferred) &&
          !(this.#activePreferredMicrophoneId === preferred && this.#activePreferredUnavailable)
        ) {
          if (this.#activePreferredMicrophoneId === preferred) {
            this.#activeExplicitDeviceAbsent = false;
          }
          this.#setWelcomeMicrophoneBindingKnown(true);
        }
        return;
      }
      if (this.#activePreferredMicrophoneId === preferred) {
        this.#activeExplicitDeviceAbsent = true;
        this.#invalidateActiveTestEvidence();
      }
      this.#notifyMicrophoneUnavailable();
    });
  }

  #setWelcomeMicrophoneBindingKnown(known: boolean): void {
    if (this.#welcomeMicrophoneBindingKnown === known) return;
    this.#welcomeMicrophoneBindingKnown = known;
    try {
      this.#onMicrophoneValidationChanged?.(known);
    } catch {
      // Recording ownership remains authoritative if Welcome validation fails.
    }
    this.#publishDeviceSnapshot();
  }

  #invalidateDefaultEvidence(): void {
    this.#invalidateActiveTestEvidence();
    this.#notifyMicrophoneUnavailable();
  }

  #queueDefaultRebind(bindingGeneration: number): void {
    if (
      (this.#activePreferredMicrophoneId !== null && !this.#activePreferredUnavailable) ||
      bindingGeneration < this.#activeBindingGeneration
    ) {
      return;
    }
    if (this.#defaultRebindAttemptGeneration === bindingGeneration) {
      this.#defaultRebindFollowUp = true;
      return;
    }
    if (
      this.#pendingDefaultRebindGeneration === null ||
      bindingGeneration > this.#pendingDefaultRebindGeneration
    ) {
      this.#pendingDefaultRebindGeneration = bindingGeneration;
    }
    if (this.#activeCaptureActivated) this.#startDefaultRebindDrain();
  }

  async getDevices(): Promise<MicrophoneDeviceList> {
    await this.#refreshDevices();
    return this.#deviceSnapshot();
  }

  getState(): MicrophoneTestState {
    return structuredClone(this.#state);
  }

  microphoneTestObservation(): {
    readonly boundDeviceId: string | null;
    readonly observedRms: number;
    readonly sampleCount: number;
  } | null {
    if (
      this.#state.status !== 'active' ||
      this.#activeCaptureKind !== 'test' ||
      this.#activeCaptureId !== this.#state.captureId ||
      this.#state.preferredUnavailable ||
      this.#ownerWebContents === null ||
      this.#activeBindingGeneration !== this.#state.bindingGeneration ||
      this.#settings.get().recording.preferredMicrophoneId !== this.#activePreferredMicrophoneId ||
      (this.#activePreferredMicrophoneId !== null &&
        (this.#state.activeMicrophoneId !== this.#activePreferredMicrophoneId ||
          this.#activeExplicitDeviceAbsent))
    ) {
      return null;
    }
    return {
      boundDeviceId: this.#state.activeMicrophoneId,
      observedRms: this.#testObservedRms,
      sampleCount: this.#testSampleCount,
    };
  }

  async startTest(ownerWebContents: WebContents | null): Promise<MicrophoneTestState> {
    if (
      this.#pendingDictationGeneration !== null ||
      this.#activeCaptureKind === 'dictation' ||
      this.#dictation !== null
    ) {
      return {
        status: 'unavailable',
        permission: this.#permission.getStatus(),
        reason: 'capture-unavailable',
      };
    }
    const operationGeneration = ++this.#operationGeneration;
    const previousStop = this.#stopActive();
    await this.#enqueue(async () => {
      if (this.#disposed || operationGeneration !== this.#operationGeneration) return;
      const previousStopped = await previousStop;
      if (!previousStopped || operationGeneration !== this.#operationGeneration) return;
      const captureWebContents = this.#captureWebContents;
      if (
        captureWebContents === null ||
        captureWebContents.isDestroyed() ||
        ownerWebContents === null ||
        ownerWebContents.isDestroyed()
      ) {
        this.#setState({
          status: 'unavailable',
          permission: this.#permission.getStatus(),
          reason: 'capture-unavailable',
        });
        return;
      }
      const status = this.#permission.getStatus();
      if (status === 'denied' || status === 'restricted') {
        this.#setState({ status: 'blocked', permission: status, reason: 'microphone-permission' });
        return;
      }
      this.#setOwner(ownerWebContents);
      this.#setState({ status: 'starting', permission: status });
      const captureId = randomUUID();
      const preferredMicrophoneId = this.#settings.get().recording.preferredMicrophoneId;
      this.#activeCaptureId = captureId;
      this.#activeCaptureKind = 'test';
      this.#activePreferredMicrophoneId = preferredMicrophoneId;
      this.#activeBindingGeneration = 0;
      this.#activeCaptureActivated = false;
      this.#activeExplicitDeviceAbsent = false;
      this.#activePreferredUnavailable = false;
      this.#permission.authorize(
        captureWebContents.id,
        captureId,
        preferredMicrophoneId === null ? 1 : 2,
      );
      try {
        const started = await this.#capture.start(preferredMicrophoneId, captureId);
        if (
          operationGeneration !== this.#operationGeneration ||
          !this.#hasOwner(ownerWebContents.id)
        ) {
          await this.#stopActive();
          return;
        }
        this.#activeBindingGeneration = started.bindingGeneration;
        this.#activePreferredUnavailable =
          preferredMicrophoneId !== null && started.preferredUnavailable;
        if (this.#activePreferredUnavailable) {
          this.#explicitValidationGeneration += 1;
          this.#activeExplicitDeviceAbsent = true;
          this.#setWelcomeMicrophoneBindingKnown(false);
          this.#notifyMicrophoneUnavailable();
        } else if (
          preferredMicrophoneId === null ||
          started.activeMicrophoneId === preferredMicrophoneId
        ) {
          if (preferredMicrophoneId !== null) this.#explicitValidationGeneration += 1;
          this.#activeExplicitDeviceAbsent = false;
          this.#setWelcomeMicrophoneBindingKnown(true);
        }
        const activated = await this.#activateWithDeviceRefresh(
          captureWebContents.id,
          captureId,
          operationGeneration,
        );
        if (!activated || !this.#hasOwner(ownerWebContents.id)) {
          await this.#stopActive();
          return;
        }
        this.#lastLevelEventAt = 0;
        this.#testObservedRms = 0;
        this.#testSampleCount = 0;
        this.#setState({
          status: 'active',
          permission: 'granted',
          captureId,
          activeMicrophoneId: started.activeMicrophoneId,
          preferredUnavailable: started.preferredUnavailable,
          bindingGeneration: started.bindingGeneration,
          sampleRate: started.sampleRate,
          channelCount: started.channelCount,
        });
      } catch (error: unknown) {
        this.#invalidateEvidenceForStartupFailure(error);
        const safelyStopped = await this.#stopActive();
        if (safelyStopped && operationGeneration === this.#operationGeneration) {
          this.#setFailureState(error, captureId);
        }
      }
    });
    return this.getState();
  }

  async startDictation(
    callbacks: DictationCaptureCallbacks,
    options: DictationCaptureOptions = {},
  ): Promise<DictationCapture> {
    const includeSystemAudio = options.includeSystemAudio === true;
    const operationGeneration = ++this.#operationGeneration;
    this.#pendingDictationGeneration = operationGeneration;
    const previousStop = this.#stopActive();
    const result: { value: DictationCapture | null } = { value: null };
    try {
      await this.#enqueue(async () => {
        if (this.#disposed || operationGeneration !== this.#operationGeneration) return;
        if (!(await previousStop) || operationGeneration !== this.#operationGeneration) return;
        if (this.#state.status === 'active' || this.#state.status === 'starting') {
          this.#setState({ status: 'idle', permission: this.#permission.getStatus() });
        }
        const captureWebContents = this.#captureWebContents;
        if (captureWebContents === null || captureWebContents.isDestroyed()) {
          throw new CaptureClientError('capture-unavailable');
        }
        const status = this.#permission.getStatus();
        if (status === 'denied' || status === 'restricted') {
          throw new CaptureClientError('permission-denied');
        }
        const captureId = randomUUID();
        const preferredMicrophoneId = this.#settings.get().recording.preferredMicrophoneId;
        this.#activeCaptureId = captureId;
        this.#activeCaptureKind = 'dictation';
        this.#activePreferredMicrophoneId = preferredMicrophoneId;
        this.#activeBindingGeneration = 0;
        this.#activeCaptureActivated = false;
        this.#activeExplicitDeviceAbsent = false;
        this.#activePreferredUnavailable = false;
        this.#permission.authorize(
          captureWebContents.id,
          captureId,
          preferredMicrophoneId === null ? 1 : 2,
        );
        try {
          if (includeSystemAudio) {
            if (this.#systemAudio?.supported !== true) {
              throw new CaptureClientError('system-audio-unavailable');
            }
            this.#systemAudio.authorize(captureWebContents, captureId);
          }
          const started = await this.#capture.start(
            preferredMicrophoneId,
            captureId,
            includeSystemAudio,
          );
          if (operationGeneration !== this.#operationGeneration) {
            await this.#stopActive();
            return;
          }
          const dictation: ActiveDictation = {
            captureId,
            activeMicrophoneId: started.activeMicrophoneId,
            preferredUnavailable: started.preferredUnavailable,
            bindingGeneration: started.bindingGeneration,
            callbacks,
          };
          this.#dictation = dictation;
          this.#activeBindingGeneration = started.bindingGeneration;
          this.#activePreferredUnavailable =
            preferredMicrophoneId !== null && started.preferredUnavailable;
          if (this.#activePreferredUnavailable) {
            this.#explicitValidationGeneration += 1;
            this.#activeExplicitDeviceAbsent = true;
            this.#setWelcomeMicrophoneBindingKnown(false);
            this.#notifyMicrophoneUnavailable();
          } else if (
            preferredMicrophoneId === null ||
            started.activeMicrophoneId === preferredMicrophoneId
          ) {
            if (preferredMicrophoneId !== null) this.#explicitValidationGeneration += 1;
            this.#activeExplicitDeviceAbsent = false;
            this.#setWelcomeMicrophoneBindingKnown(true);
          }
          const activated = await this.#activateWithDeviceRefresh(
            captureWebContents.id,
            captureId,
            operationGeneration,
          );
          if (!activated || this.#dictation !== dictation) {
            await this.#stopActive();
            return;
          }
          result.value = {
            captureId,
            activeMicrophoneId: started.activeMicrophoneId,
            preferredUnavailable: started.preferredUnavailable,
          };
        } catch (error: unknown) {
          this.#invalidateEvidenceForStartupFailure(error);
          const policyDenied =
            error instanceof CaptureClientError &&
            error.code === 'permission-denied' &&
            this.#permission.takePolicyDenial(captureId);
          await this.#stopActive();
          if (policyDenied) {
            console.error('Talking Quill microphone request rejected by application policy', {
              code: 'MICROPHONE_POLICY_DENIED',
            });
            throw new CaptureClientError('capture-unavailable');
          }
          throw error;
        }
      });
    } finally {
      if (this.#pendingDictationGeneration === operationGeneration) {
        this.#pendingDictationGeneration = null;
      }
    }
    if (result.value === null) throw new CaptureClientError('capture-unavailable');
    return result.value;
  }

  async stopDictation(captureId?: string): Promise<void> {
    const activeDictationId =
      this.#activeCaptureKind === 'dictation' ? this.#activeCaptureId : null;
    const drainingDictationId = this.#drainingDictation?.captureId ?? null;
    if (captureId !== undefined) {
      if (captureId === drainingDictationId) {
        await this.#stopInFlight?.promise;
        return;
      }
      if (captureId !== activeDictationId) return;
    } else if (activeDictationId === null) {
      if (this.#pendingDictationGeneration !== null) {
        const priorStop = this.#stopInFlight?.promise;
        const cancellationGeneration = ++this.#operationGeneration;
        this.#pendingDictationGeneration = null;
        const safelyStopped = priorStop === undefined || (await priorStop);
        if (
          safelyStopped &&
          cancellationGeneration === this.#operationGeneration &&
          (this.#state.status === 'active' || this.#state.status === 'starting')
        ) {
          this.#setState({ status: 'idle', permission: this.#permission.getStatus() });
        }
      } else if (drainingDictationId !== null) {
        await this.#stopInFlight?.promise;
      }
      return;
    }
    ++this.#operationGeneration;
    this.#pendingDictationGeneration = null;
    await this.#stopActive();
  }

  async stopTest(ownerWebContentsId?: number): Promise<MicrophoneTestState> {
    if (
      this.#pendingDictationGeneration !== null ||
      this.#activeCaptureKind === 'dictation' ||
      this.#dictation !== null
    ) {
      return this.getState();
    }
    if (
      ownerWebContentsId !== undefined &&
      this.#ownerWebContents !== null &&
      ownerWebContentsId !== this.#ownerWebContents.id
    ) {
      return this.getState();
    }
    const operationGeneration = ++this.#operationGeneration;
    const safelyStopped = await this.#stopActive();
    if (safelyStopped && operationGeneration === this.#operationGeneration) {
      this.#setState({ status: 'idle', permission: this.#permission.getStatus() });
    }
    return this.getState();
  }

  async openMicrophoneSettings(): Promise<void> {
    await this.#permission.openSettings();
  }

  shutdown(): Promise<void> {
    if (this.#shutdownPromise !== null) return this.#shutdownPromise;
    this.#disposed = true;
    ++this.#operationGeneration;
    ++this.#deviceRefreshGeneration;
    this.#deviceRefreshDirty = false;
    this.#pendingDefaultRebindGeneration = null;
    this.#defaultRebindAttemptGeneration = null;
    this.#defaultRebindFollowUp = false;
    for (const waiter of this.#deviceRefreshWaiters) waiter.resolve();
    this.#deviceRefreshWaiters = [];
    const stopping = this.#stopActive();
    this.#shutdownPromise = (async () => {
      await this.#enqueue(async () => {
        await stopping;
      });
      this.#removeFrameListener();
      this.#removeDeviceListener();
      this.#removeDefaultInvalidationListener();
      this.#removeStopListener();
      this.#permission.releaseAll();
      this.#systemAudio?.releaseAll();
      this.#onMicrophoneUnavailable = null;
      this.#onMicrophoneValidationChanged = null;
      this.#capture.dispose();
    })();
    return this.#shutdownPromise;
  }

  async #activateWithDeviceRefresh(
    webContentsId: number,
    captureId: string,
    operationGeneration: number,
  ): Promise<boolean> {
    this.#permission.seal(captureId);
    await this.#capture.activate(captureId);
    if (
      this.#disposed ||
      operationGeneration !== this.#operationGeneration ||
      captureId !== this.#activeCaptureId
    ) {
      return false;
    }
    this.#activeCaptureActivated = true;
    this.#startDefaultRebindDrain();
    void this.#refreshDevices({ webContentsId, captureId, operationGeneration });
    return true;
  }

  #refreshDevices(authorized?: AuthorizedDeviceRefresh): Promise<void> {
    if (this.#captureWebContents === null || this.#disposed) return Promise.resolve();
    const generation = ++this.#deviceRefreshGeneration;
    this.#deviceRefreshDirty = true;
    if (authorized !== undefined) this.#pendingAuthorizedDeviceRefresh = authorized;
    const result = new Promise<void>((resolve) => {
      this.#deviceRefreshWaiters.push({ generation, resolve });
    });
    this.#startDeviceRefreshDrain();
    return result;
  }

  #startDeviceRefreshDrain(): void {
    if (this.#deviceRefreshInFlight !== null || this.#disposed) return;
    const drain = this.#drainDeviceRefreshes();
    this.#deviceRefreshInFlight = drain;
    void drain.finally(() => {
      if (this.#deviceRefreshInFlight === drain) this.#deviceRefreshInFlight = null;
      if (this.#deviceRefreshDirty) this.#startDeviceRefreshDrain();
    });
  }

  async #drainDeviceRefreshes(): Promise<void> {
    while (this.#deviceRefreshDirty && !this.#disposed) {
      this.#deviceRefreshDirty = false;
      const refreshGeneration = this.#deviceRefreshGeneration;
      const attachmentGeneration = this.#captureAttachmentGeneration;
      const captureWebContents = this.#captureWebContents;
      const authorized = this.#pendingAuthorizedDeviceRefresh;
      this.#pendingAuthorizedDeviceRefresh = null;
      let result: Readonly<{ devices: readonly MicrophoneDevice[]; authorized: boolean }> | null =
        null;
      if (captureWebContents !== null) {
        try {
          if (authorized !== null && this.#authorizedRefreshIsCurrent(authorized)) {
            result = await this.#withPermissionOperation(async () => {
              if (!this.#authorizedRefreshIsCurrent(authorized)) return null;
              this.#permission.authorizeEnumeration(authorized.webContentsId, authorized.captureId);
              try {
                return { devices: await this.#capture.listDevices(), authorized: true } as const;
              } finally {
                this.#permission.seal(authorized.captureId);
              }
            });
          } else {
            result = await this.#withPermissionOperation(async () => ({
              devices: await this.#capture.listDevices(),
              authorized: false,
            }));
          }
        } catch {
          // Failed enumeration never replaces the last known authorized snapshot.
        }
      }
      if (
        result?.authorized === true &&
        refreshGeneration !== this.#deviceRefreshGeneration &&
        authorized !== null &&
        this.#authorizedRefreshIsCurrent(authorized)
      ) {
        this.#retainAuthorizedDeviceRefresh(authorized);
      }
      const refreshIsCurrent = refreshGeneration === this.#deviceRefreshGeneration;
      if (
        result !== null &&
        refreshIsCurrent &&
        (!result.authorized || this.#authorizedRefreshIsCurrent(authorized)) &&
        attachmentGeneration === this.#captureAttachmentGeneration &&
        captureWebContents === this.#captureWebContents
      ) {
        if (result.authorized) {
          this.#lastAuthorizedDeviceRefreshGeneration = refreshGeneration;
          this.#devices = result.devices;
          const preferred = this.#settings.get().recording.preferredMicrophoneId;
          if (preferred !== null) {
            const present = result.devices.some((device) => device.deviceId === preferred);
            const fallbackBinding =
              this.#activePreferredMicrophoneId === preferred && this.#activePreferredUnavailable;
            if (this.#activePreferredMicrophoneId === preferred) {
              this.#activeExplicitDeviceAbsent = fallbackBinding || !present;
            }
            if (fallbackBinding) {
              this.#setWelcomeMicrophoneBindingKnown(false);
            } else if (present) {
              this.#setWelcomeMicrophoneBindingKnown(true);
            } else {
              this.#setWelcomeMicrophoneBindingKnown(false);
              this.#invalidateActiveTestEvidence();
              this.#notifyMicrophoneUnavailable();
            }
          }
        }
        this.#publishDeviceSnapshot();
      }
      if (refreshIsCurrent) this.#resolveDeviceRefreshWaiters(refreshGeneration);
    }
  }

  #retainAuthorizedDeviceRefresh(refresh: AuthorizedDeviceRefresh): void {
    this.#pendingAuthorizedDeviceRefresh ??= refresh;
  }

  #authorizedRefreshIsCurrent(refresh: AuthorizedDeviceRefresh | null): boolean {
    return (
      refresh !== null &&
      !this.#disposed &&
      refresh.operationGeneration === this.#operationGeneration &&
      refresh.captureId === this.#activeCaptureId &&
      this.#captureWebContents?.id === refresh.webContentsId
    );
  }

  #resolveDeviceRefreshWaiters(generation: number): void {
    const pending: DeviceRefreshWaiter[] = [];
    for (const waiter of this.#deviceRefreshWaiters) {
      if (waiter.generation <= generation) waiter.resolve();
      else pending.push(waiter);
    }
    this.#deviceRefreshWaiters = pending;
  }

  #startDefaultRebindDrain(): void {
    if (
      this.#defaultRebindInFlight !== null ||
      this.#pendingDefaultRebindGeneration === null ||
      this.#disposed
    ) {
      return;
    }
    const drain = this.#drainDefaultRebinds();
    this.#defaultRebindInFlight = drain;
    void drain.finally(() => {
      if (this.#defaultRebindInFlight === drain) this.#defaultRebindInFlight = null;
      if (this.#pendingDefaultRebindGeneration !== null) this.#startDefaultRebindDrain();
    });
  }

  async #drainDefaultRebinds(): Promise<void> {
    while (this.#pendingDefaultRebindGeneration !== null && !this.#disposed) {
      const bindingGeneration = this.#pendingDefaultRebindGeneration;
      this.#pendingDefaultRebindGeneration = null;
      const captureId = this.#activeCaptureId;
      const captureWebContents = this.#captureWebContents;
      const operationGeneration = this.#operationGeneration;
      if (
        captureId === null ||
        captureWebContents === null ||
        captureWebContents.isDestroyed() ||
        !this.#activeCaptureActivated ||
        (this.#activePreferredMicrophoneId !== null && !this.#activePreferredUnavailable) ||
        bindingGeneration !== this.#activeBindingGeneration
      ) {
        continue;
      }
      this.#defaultRebindAttemptGeneration = bindingGeneration;
      let failedAttempts = 0;
      try {
        while (failedAttempts < DEFAULT_MICROPHONE_REBIND_ATTEMPTS) {
          try {
            const rebound = await this.#withPermissionOperation(async () => {
              if (
                this.#disposed ||
                operationGeneration !== this.#operationGeneration ||
                captureId !== this.#activeCaptureId ||
                bindingGeneration !== this.#activeBindingGeneration
              ) {
                throw new CaptureClientError('capture-unavailable');
              }
              this.#permission.authorize(captureWebContents.id, captureId);
              try {
                return await this.#capture.rebindDefault(captureId, bindingGeneration);
              } finally {
                this.#permission.seal(captureId);
              }
            });
            if (
              operationGeneration !== this.#operationGeneration ||
              captureId !== this.#activeCaptureId ||
              bindingGeneration !== this.#activeBindingGeneration
            ) {
              return;
            }
            if (rebound.bindingGeneration !== bindingGeneration + 1) {
              throw new CaptureClientError('capture-failed');
            }
            this.#activeBindingGeneration = rebound.bindingGeneration;
            if (this.#dictation?.captureId === captureId) {
              this.#dictation = {
                ...this.#dictation,
                activeMicrophoneId: rebound.activeMicrophoneId,
                bindingGeneration: rebound.bindingGeneration,
              };
            }
            if (this.#state.status === 'active' && this.#state.captureId === captureId) {
              this.#invalidateActiveTestEvidence();
              this.#setState({
                ...this.#state,
                activeMicrophoneId: rebound.activeMicrophoneId,
                bindingGeneration: rebound.bindingGeneration,
              });
            }
            this.#notifyMicrophoneUnavailable();
            if (this.#defaultRebindFollowUp) {
              this.#defaultRebindFollowUp = false;
              this.#pendingDefaultRebindGeneration = rebound.bindingGeneration;
            }
            break;
          } catch {
            if (
              operationGeneration !== this.#operationGeneration ||
              captureId !== this.#activeCaptureId
            ) {
              return;
            }
            failedAttempts += 1;
            if (failedAttempts >= DEFAULT_MICROPHONE_REBIND_ATTEMPTS) {
              await this.#failDefaultRebind(captureId);
              return;
            }
          }
        }
      } finally {
        if (this.#defaultRebindAttemptGeneration === bindingGeneration) {
          this.#defaultRebindAttemptGeneration = null;
        }
      }
    }
  }

  async #failDefaultRebind(captureId: string): Promise<void> {
    if (captureId !== this.#activeCaptureId) return;
    const dictation = this.#dictation;
    const wasTest = this.#activeCaptureKind === 'test';
    const failureGeneration = ++this.#operationGeneration;
    this.#pendingDefaultRebindGeneration = null;
    this.#defaultRebindFollowUp = false;
    await this.#stopActive();
    if (failureGeneration !== this.#operationGeneration) return;
    if (dictation !== null) {
      try {
        dictation.callbacks.onUnexpectedStop('device-unavailable');
      } catch {
        // Capture ownership is already released; consumer failure cannot undo cleanup.
      }
    } else if (wasTest) {
      this.#setState({
        status: 'unavailable',
        permission: this.#permission.getStatus(),
        reason: 'device-unavailable',
      });
    }
  }

  #withPermissionOperation<Result>(operation: () => Promise<Result>): Promise<Result> {
    const result = this.#permissionOperation.then(operation, operation);
    this.#permissionOperation = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  #stopActive(): Promise<boolean> {
    const inFlight = this.#stopInFlight;
    if (inFlight !== null) return inFlight.promise;
    const captureId = this.#activeCaptureId;
    const dictation = this.#dictation;
    this.#activeCaptureKind = null;
    this.#activePreferredMicrophoneId = null;
    this.#activeCaptureActivated = false;
    this.#activeExplicitDeviceAbsent = false;
    this.#activePreferredUnavailable = false;
    this.#pendingDefaultRebindGeneration = null;
    this.#defaultRebindAttemptGeneration = null;
    this.#defaultRebindFollowUp = false;
    this.#dictation = null;
    this.#clearOwner();
    if (captureId === null) return Promise.resolve(true);
    this.#activeCaptureId = null;
    if (dictation?.captureId === captureId) this.#drainingDictation = dictation;
    this.#permission.release(captureId);
    this.#systemAudio?.release(captureId);

    const promise = this.#stopCapture(captureId);
    const stop = { promise };
    this.#stopInFlight = stop;
    const clearStop = () => {
      if (this.#stopInFlight === stop) this.#stopInFlight = null;
      if (this.#drainingDictation?.captureId === captureId) this.#drainingDictation = null;
    };
    void promise.then(clearStop, clearStop);
    return promise;
  }

  async #stopCapture(captureId: string): Promise<boolean> {
    let resolveTimeout!: (value: false) => void;
    const timeout = new Promise<false>((resolve) => {
      resolveTimeout = resolve;
    });
    const timer = setTimeout(() => resolveTimeout(false), CAPTURE_CANCEL_TIMEOUT_MS);
    timer.unref();
    const stopped = await Promise.race([
      this.#capture.stop(captureId).then(
        () => true as const,
        () => false as const,
      ),
      timeout,
    ]);
    clearTimeout(timer);
    if (stopped) return true;
    this.#forceCaptureReset(captureId);
    return false;
  }

  #forceCaptureReset(captureId: string): void {
    try {
      this.#capture.reset();
    } catch {
      // Continue clearing local ownership even if the failed transport cannot be reset cleanly.
    }
    const captureWebContents = this.#captureWebContents;
    this.#captureWebContents = null;
    if (captureWebContents !== null && !captureWebContents.isDestroyed()) {
      try {
        captureWebContents.reload();
      } catch {
        // The capture renderer may disappear between the destruction check and reload.
      }
    }
    this.#activeCaptureId = null;
    this.#activeCaptureKind = null;
    this.#activePreferredMicrophoneId = null;
    this.#activeCaptureActivated = false;
    this.#activeExplicitDeviceAbsent = false;
    this.#activePreferredUnavailable = false;
    this.#pendingDefaultRebindGeneration = null;
    this.#defaultRebindAttemptGeneration = null;
    this.#defaultRebindFollowUp = false;
    this.#dictation = null;
    this.#drainingDictation = null;
    this.#clearOwner();
    this.#permission.release(captureId);
    this.#systemAudio?.release(captureId);
    this.#setState({
      status: 'unavailable',
      permission: this.#permission.getStatus(),
      reason: 'capture-unavailable',
    });
  }

  #deviceSnapshot(): MicrophoneDeviceList {
    const preferredMicrophoneId = this.#settings.get().recording.preferredMicrophoneId;
    return {
      devices: [...this.#devices],
      preferredMicrophoneId,
      preferredAvailable:
        preferredMicrophoneId === null ||
        (this.#welcomeMicrophoneBindingKnown &&
          (this.#devices.some((device) => device.deviceId === preferredMicrophoneId) ||
            (this.#activePreferredMicrophoneId === preferredMicrophoneId &&
              !this.#activeExplicitDeviceAbsent))),
      permission: this.#permission.getStatus(),
    };
  }

  #publishDeviceSnapshot(): void {
    const snapshot = this.#deviceSnapshot();
    if (deviceSnapshotsEqual(this.#lastPublishedDeviceSnapshot, snapshot)) return;
    this.#lastPublishedDeviceSnapshot = snapshot;
    this.#events.send('recording:devices-changed', snapshot);
  }

  #hasOwner(ownerId: number): boolean {
    return this.#ownerWebContents?.id === ownerId;
  }

  #setOwner(owner: WebContents): void {
    this.#clearOwner();
    this.#ownerWebContents = owner;
    owner.once('destroyed', this.#onOwnerDestroyed);
    owner.on('did-start-navigation', this.#onOwnerDidStartNavigation);
    owner.once('render-process-gone', this.#onOwnerRenderProcessGone);
  }

  #clearOwner(): void {
    this.#ownerWebContents?.removeListener('destroyed', this.#onOwnerDestroyed);
    this.#ownerWebContents?.removeListener('did-start-navigation', this.#onOwnerDidStartNavigation);
    this.#ownerWebContents?.removeListener('render-process-gone', this.#onOwnerRenderProcessGone);
    this.#ownerWebContents = null;
  }

  #stopForOwnerLifecycle(): void {
    const ownerId = this.#ownerWebContents?.id;
    if (ownerId !== undefined) void this.stopTest(ownerId);
  }

  readonly #onOwnerDestroyed = () => this.#stopForOwnerLifecycle();
  readonly #onOwnerRenderProcessGone = () => this.#stopForOwnerLifecycle();
  readonly #onOwnerDidStartNavigation = (
    _event: Electron.Event,
    _url: string,
    _isInPlace: boolean,
    isMainFrame: boolean,
  ) => {
    if (isMainFrame) this.#stopForOwnerLifecycle();
  };

  #invalidateEvidenceForStartupFailure(error: unknown): void {
    if (
      error instanceof CaptureClientError &&
      (error.code === 'no-device' || error.code === 'device-unavailable')
    ) {
      this.#setWelcomeMicrophoneBindingKnown(false);
      this.#notifyMicrophoneUnavailable();
    }
  }

  #setFailureState(error: unknown, captureId: string): void {
    const code = error instanceof CaptureClientError ? error.code : 'capture-failed';
    if (code === 'permission-denied') {
      if (this.#permission.takePolicyDenial(captureId)) {
        console.error('Talking Quill microphone request rejected by application policy', {
          code: 'MICROPHONE_POLICY_DENIED',
        });
        this.#setState({
          status: 'unavailable',
          permission: this.#permission.getStatus(),
          reason: 'permission-unavailable',
        });
        return;
      }
      const permission = this.#permission.getStatus();
      if (permission === 'denied' || permission === 'restricted') {
        this.#setState({ status: 'blocked', permission, reason: 'microphone-permission' });
      } else {
        this.#setState({
          status: 'unavailable',
          permission,
          reason: 'permission-unavailable',
        });
      }
      return;
    }
    const reason =
      code === 'no-device'
        ? 'no-device'
        : code === 'device-unavailable'
          ? 'device-unavailable'
          : code === 'unsupported-audio-format'
            ? 'unsupported-audio-format'
            : 'capture-unavailable';
    this.#setState({
      status: 'unavailable',
      permission: this.#permission.getStatus(),
      reason,
    });
  }

  #invalidateActiveTestEvidence(): void {
    if (this.#activeCaptureKind !== 'test' || this.#state.status !== 'active') return;
    this.#lastLevelEventAt = 0;
    this.#testObservedRms = 0;
    this.#testSampleCount = 0;
    try {
      this.#events.send('recording:test-level', {
        captureId: this.#state.captureId,
        rms: 0,
      });
    } catch {
      // The test metadata remains authoritative if its renderer disappeared.
    }
  }

  #notifyMicrophoneUnavailable(): void {
    try {
      this.#onMicrophoneUnavailable?.();
    } catch {
      // Evidence invalidation is ancillary to releasing the failed capture.
    }
  }

  #setState(state: MicrophoneTestState): void {
    this.#state = state;
    try {
      this.#events.send('recording:test-state-changed', state);
    } catch {
      // State remains authoritative if its renderer disappeared during publication.
    }
  }

  async #enqueue(operation: () => Promise<void>): Promise<void> {
    const next = this.#operation.then(operation, operation);
    this.#operation = next.catch(() => undefined);
    await next;
  }
}

function deviceSnapshotsEqual(
  first: MicrophoneDeviceList | null,
  second: MicrophoneDeviceList,
): boolean {
  if (
    first?.preferredMicrophoneId !== second.preferredMicrophoneId ||
    first.preferredAvailable !== second.preferredAvailable ||
    first.permission !== second.permission ||
    first.devices.length !== second.devices.length
  ) {
    return false;
  }
  return first.devices.every((device, index) => {
    const other = second.devices[index];
    return (
      device.deviceId === other?.deviceId &&
      device.label === other.label &&
      device.isDefault === other.isDefault
    );
  });
}
