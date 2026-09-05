import type { WebContents } from 'electron';
import type { MicrophoneDeviceList, MicrophoneTestState } from '../../shared/schemas/audio';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import type { MicrophonePermissionController } from '../security/microphone-permission';
import type { SystemAudioCaptureController } from '../security/system-audio-capture';
import type { CaptureWindowClient } from './capture-window-client';
import {
  type DictationCapture,
  type DictationCaptureCallbacks,
  type DictationCaptureOptions,
  RecordingContext,
} from './recording-context';
import { getDevices } from './recording-devices';
import { startDictation, stopDictation } from './recording-dictation';
import {
  microphoneReadyForWelcome,
  setWelcomeEvidenceInvalidator,
  setWelcomeEvidenceValidationListener,
} from './recording-evidence';
import { invalidateInputDevices } from './recording-input-invalidation';
import { attachCapture, openMicrophoneSettings, shutdown } from './recording-lifecycle';
import { initializeListeners } from './recording-listeners';
import { getState, microphoneTestObservation, startTest, stopTest } from './recording-test';

export type {
  DictationCapture,
  DictationCaptureCallbacks,
  DictationCaptureOptions,
} from './recording-context';

export class RecordingService {
  readonly #context: RecordingContext;
  constructor(
    capture: CaptureWindowClient,
    settings: SettingsStore,
    events: IpcEventEmitter,
    permission: MicrophonePermissionController,
    systemAudio: SystemAudioCaptureController | null = null,
  ) {
    const context = new RecordingContext(capture, settings, events, permission, systemAudio);
    this.#context = context;
    initializeListeners(context);
  }
  setWelcomeEvidenceInvalidator(listener: () => void): void {
    const context = this.#context;
    return setWelcomeEvidenceInvalidator(context, listener);
  }
  setWelcomeEvidenceValidationListener(listener: (known: boolean) => void): void {
    const context = this.#context;
    return setWelcomeEvidenceValidationListener(context, listener);
  }
  microphoneReadyForWelcome(): boolean {
    const context = this.#context;
    return microphoneReadyForWelcome(context);
  }
  attachCapture(webContents: WebContents): void {
    const context = this.#context;
    return attachCapture(context, webContents);
  }
  invalidateInputDevices(): void {
    const context = this.#context;
    return invalidateInputDevices(context);
  }
  getDevices(): Promise<MicrophoneDeviceList> {
    const context = this.#context;
    return getDevices(context);
  }
  getState(): MicrophoneTestState {
    const context = this.#context;
    return getState(context);
  }
  microphoneTestObservation(): {
    readonly boundDeviceId: string | null;
    readonly observedRms: number;
    readonly sampleCount: number;
  } | null {
    const context = this.#context;
    return microphoneTestObservation(context);
  }
  startTest(
    ownerWebContents: WebContents | null,
    signal?: AbortSignal,
  ): Promise<MicrophoneTestState> {
    const context = this.#context;
    return startTest(context, ownerWebContents, signal);
  }
  startDictation(
    callbacks: DictationCaptureCallbacks,
    options: DictationCaptureOptions = {},
  ): Promise<DictationCapture> {
    const context = this.#context;
    return startDictation(context, callbacks, options);
  }
  stopDictation(captureId?: string): Promise<void> {
    const context = this.#context;
    return stopDictation(context, captureId);
  }
  stopTest(ownerWebContentsId?: number, signal?: AbortSignal): Promise<MicrophoneTestState> {
    const context = this.#context;
    return stopTest(context, ownerWebContentsId, signal);
  }
  openMicrophoneSettings(): Promise<void> {
    const context = this.#context;
    return openMicrophoneSettings(context);
  }
  shutdown(): Promise<void> {
    const context = this.#context;
    return shutdown(context);
  }
}
