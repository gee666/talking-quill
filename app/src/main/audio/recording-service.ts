import type { WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import { CAPTURE_CANCEL_TIMEOUT_MS } from '../../shared/constants/audio';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import type { MicrophonePermissionController } from '../security/microphone-permission';
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
  readonly onUnexpectedStop: (reason: 'device-unavailable' | 'capture-unavailable') => void;
}

export interface DictationCapture {
  readonly captureId: string;
  readonly activeMicrophoneId: string | null;
}

interface ActiveDictation extends DictationCapture {
  readonly callbacks: DictationCaptureCallbacks;
}

export class RecordingService {
  readonly #capture: CaptureWindowClient;
  readonly #settings: SettingsStore;
  readonly #events: IpcEventEmitter;
  readonly #permission: MicrophonePermissionController;
  readonly #removeFrameListener: () => void;
  readonly #removeDeviceListener: () => void;
  readonly #removeStopListener: () => void;
  #captureWebContents: WebContents | null = null;
  #activeCaptureId: string | null = null;
  #dictation: ActiveDictation | null = null;
  #ownerWebContents: WebContents | null = null;
  #devices: readonly MicrophoneDevice[] = [];
  #state: MicrophoneTestState;
  #lastLevelEventAt = 0;
  #testObservedRms = 0;
  #testSampleCount = 0;
  #operation: Promise<void> = Promise.resolve();
  #operationGeneration = 0;
  #disposed = false;
  #onMicrophoneUnavailable: (() => void) | null = null;

  constructor(
    capture: CaptureWindowClient,
    settings: SettingsStore,
    events: IpcEventEmitter,
    permission: MicrophonePermissionController,
  ) {
    this.#capture = capture;
    this.#settings = settings;
    this.#events = events;
    this.#permission = permission;
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
      if (frame.captureId !== this.#activeCaptureId) return;
      if (this.#dictation?.captureId === frame.captureId) {
        this.#dictation.callbacks.onFrame(frame.samples, frame.rms);
        return;
      }
      if (this.#state.status !== 'active') return;
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
    this.#removeDeviceListener = capture.onDevicesChanged((devices) => {
      // Empty enumeration may mean Electron policy withheld device metadata after the short-lived
      // explicit authorization ended. Preserve names; an active track ending still reports loss.
      if (devices.length === 0 && this.#devices.length > 0) return;
      this.#devices = devices;
      const preferred = this.#settings.get().recording.preferredMicrophoneId;
      if (preferred !== null && !devices.some((device) => device.deviceId === preferred)) {
        this.#onMicrophoneUnavailable?.();
      }
      this.#events.send('recording:devices-changed', this.#deviceSnapshot());
    });
    this.#removeStopListener = capture.onUnexpectedStop((captureId) => {
      if (captureId !== this.#activeCaptureId) return;
      const dictation = this.#dictation;
      this.#activeCaptureId = null;
      this.#dictation = null;
      this.#clearOwner();
      this.#permission.release(captureId);
      this.#onMicrophoneUnavailable?.();
      if (dictation !== null) {
        dictation.callbacks.onUnexpectedStop('device-unavailable');
        return;
      }
      this.#setState({
        status: 'unavailable',
        permission: this.#permission.getStatus(),
        reason: 'device-unavailable',
      });
    });
  }

  setWelcomeEvidenceInvalidator(listener: () => void): void {
    this.#onMicrophoneUnavailable = listener;
  }

  attachCapture(webContents: WebContents): void {
    this.#captureWebContents = webContents;
    this.#capture.attach(webContents);
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
    if (this.#state.status !== 'active') return null;
    return {
      boundDeviceId: this.#state.activeMicrophoneId,
      observedRms: this.#testObservedRms,
      sampleCount: this.#testSampleCount,
    };
  }

  async startTest(ownerWebContents: WebContents | null): Promise<MicrophoneTestState> {
    if (this.#dictation !== null) {
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
      this.#permission.authorize(
        captureWebContents.id,
        captureId,
        preferredMicrophoneId === null ? 1 : 2,
      );
      try {
        const started = await this.#capture.start(preferredMicrophoneId, captureId);
        this.#permission.authorizeEnumeration(captureWebContents.id, captureId);
        await this.#refreshDevices();
        this.#permission.seal(captureId);
        if (
          operationGeneration !== this.#operationGeneration ||
          !this.#hasOwner(ownerWebContents.id)
        ) {
          await this.#stopActive();
          return;
        }
        await this.#capture.activate(captureId);
        if (
          operationGeneration !== this.#operationGeneration ||
          !this.#hasOwner(ownerWebContents.id)
        ) {
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
          sampleRate: started.sampleRate,
          channelCount: started.channelCount,
        });
      } catch (error: unknown) {
        const safelyStopped = await this.#stopActive();
        if (safelyStopped && operationGeneration === this.#operationGeneration) {
          this.#setFailureState(error, captureId);
        }
      }
    });
    return this.getState();
  }

  async startDictation(callbacks: DictationCaptureCallbacks): Promise<DictationCapture> {
    const operationGeneration = ++this.#operationGeneration;
    const previousStop = this.#stopActive();
    const result: { value: DictationCapture | null } = { value: null };
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
      this.#permission.authorize(
        captureWebContents.id,
        captureId,
        preferredMicrophoneId === null ? 1 : 2,
      );
      try {
        const started = await this.#capture.start(preferredMicrophoneId, captureId);
        this.#permission.authorizeEnumeration(captureWebContents.id, captureId);
        await this.#refreshDevices();
        this.#permission.seal(captureId);
        if (operationGeneration !== this.#operationGeneration) {
          await this.#stopActive();
          return;
        }
        const dictation: ActiveDictation = {
          captureId,
          activeMicrophoneId: started.activeMicrophoneId,
          callbacks,
        };
        this.#dictation = dictation;
        await this.#capture.activate(captureId);
        if (operationGeneration !== this.#operationGeneration) {
          await this.#stopActive();
          return;
        }
        result.value = { captureId, activeMicrophoneId: started.activeMicrophoneId };
      } catch (error: unknown) {
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
    if (result.value === null) throw new CaptureClientError('capture-unavailable');
    return result.value;
  }

  async stopDictation(captureId?: string): Promise<void> {
    if (captureId !== undefined && this.#dictation?.captureId !== captureId) return;
    ++this.#operationGeneration;
    await this.#stopActive();
  }

  async stopTest(ownerWebContentsId?: number): Promise<MicrophoneTestState> {
    if (this.#dictation !== null) return this.getState();
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

  async shutdown(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    ++this.#operationGeneration;
    const stopping = this.#stopActive();
    await this.#enqueue(async () => {
      await stopping;
    });
    this.#removeFrameListener();
    this.#removeDeviceListener();
    this.#removeStopListener();
    this.#permission.releaseAll();
    this.#onMicrophoneUnavailable = null;
    this.#capture.dispose();
  }

  async #refreshDevices(): Promise<void> {
    if (this.#captureWebContents === null || this.#disposed) return;
    try {
      const devices = await this.#capture.listDevices();
      // Chromium returns an anonymous/empty list when enumeration has no active permission check.
      // Keep the last authorized snapshot; real hot-plug notifications still replace it directly.
      if (devices.length > 0 || this.#devices.length === 0) this.#devices = devices;
      this.#events.send('recording:devices-changed', this.#deviceSnapshot());
    } catch {
      // A background refresh must not erase the last explicitly authorized device snapshot.
    }
  }

  async #stopActive(): Promise<boolean> {
    const captureId = this.#activeCaptureId;
    this.#dictation = null;
    this.#clearOwner();
    if (captureId === null) return true;
    this.#activeCaptureId = null;
    this.#permission.release(captureId);

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
    this.#capture.reset();
    const captureWebContents = this.#captureWebContents;
    this.#captureWebContents = null;
    if (captureWebContents !== null && !captureWebContents.isDestroyed()) {
      captureWebContents.reload();
    }
    if (this.#activeCaptureId === captureId) this.#activeCaptureId = null;
    this.#clearOwner();
    this.#permission.release(captureId);
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
        this.#devices.some((device) => device.deviceId === preferredMicrophoneId),
      permission: this.#permission.getStatus(),
    };
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

  #setState(state: MicrophoneTestState): void {
    this.#state = state;
    this.#events.send('recording:test-state-changed', state);
  }

  async #enqueue(operation: () => Promise<void>): Promise<void> {
    const next = this.#operation.then(operation, operation);
    this.#operation = next.catch(() => undefined);
    await next;
  }
}
