import type { Settings } from '../../shared/schemas/settings';
import type { WindowManager } from '../app/window-manager';
import type { SilencePolicy } from '../audio/silence-policy';
import { abortOperationError } from './echo-operation';
import type {
  EchoModelUseGrant,
  EchoRecordingPort,
  EchoWhisperPort,
  WhisperStreamingSession,
} from './echo-session-ports';
import type { HelperCaptureReconciler } from './helper-capture-reconciler';
import { isCapturePhase } from './session-phase';
import type { EchoSessionEvent, EchoSessionState } from './session-reducer';

export interface CaptureSessionOwner {
  readonly generation: number;
  readonly signal: AbortSignal;
  readonly transcription: Readonly<Settings['transcription']>;
  readonly includeSystemAudio: boolean;
  active: boolean;
  captureOpening: boolean;
}

export interface EchoCaptureContextOptions {
  readonly recording: EchoRecordingPort;
  readonly whisper: EchoWhisperPort;
  readonly captureReconciler: HelperCaptureReconciler;
  readonly windows: WindowManager;
  readonly getWidgetSize: () => Settings['app']['widgetSize'];
  readonly playSound: () => void;
  readonly getState: () => EchoSessionState;
  readonly getSignal: () => AbortSignal;
  readonly abort: () => void;
  readonly dispatch: (event: EchoSessionEvent) => void;
  readonly acquireModelUse: (
    modelId: Settings['transcription']['modelId'],
    signal: AbortSignal,
  ) => Promise<EchoModelUseGrant>;
}

/** Per-instance state. The public EchoCapturePipeline owns this context. */
export class EchoCaptureContext {
  readonly recording: EchoRecordingPort;
  readonly whisper: EchoWhisperPort;
  readonly captureReconciler: HelperCaptureReconciler;
  readonly windows: WindowManager;
  readonly getWidgetSize: () => Settings['app']['widgetSize'];
  readonly playSound: () => void;
  readonly getState: () => EchoSessionState;
  readonly getSignal: () => AbortSignal;
  readonly abort: () => void;
  readonly dispatch: (event: EchoSessionEvent) => void;
  readonly acquireModelUse: (
    modelId: Settings['transcription']['modelId'],
    signal: AbortSignal,
  ) => Promise<EchoModelUseGrant>;
  generation = 0;
  sessionOwner: CaptureSessionOwner | null = null;
  captureId: string | null = null;
  captureStopCompleted = false;
  captureStopping = false;
  nativeCaptureLost = false;
  readyCuePlayed = false;
  pcmChunks: Float32Array[] = [];
  totalSamples = 0;
  streamedSamples = 0;
  discardedSamples = 0;
  streamPushPending = false;
  stream: WhisperStreamingSession | null = null;
  streamOpening: Promise<WhisperStreamingSession> | null = null;
  streamTail: Promise<void> = Promise.resolve();
  streamFailure: unknown = null;
  modelUse: EchoModelUseGrant | null = null;
  modelUseOpening: Promise<void> = Promise.resolve();
  warmupOpening: Promise<void> = Promise.resolve();
  holdTimer: ReturnType<typeof setTimeout> | null = null;
  capTimer: ReturnType<typeof setTimeout> | null = null;
  audioStartTimer: ReturnType<typeof setTimeout> | null = null;
  silence: SilencePolicy | null = null;
  pendingSilenceSubmit = false;
  lastLevelAt = 0;
  constructor(options: EchoCaptureContextOptions) {
    this.recording = options.recording;
    this.whisper = options.whisper;
    this.captureReconciler = options.captureReconciler;
    this.windows = options.windows;
    this.getWidgetSize = options.getWidgetSize;
    this.playSound = options.playSound;
    this.getState = options.getState;
    this.getSignal = options.getSignal;
    this.abort = options.abort;
    this.dispatch = options.dispatch;
    this.acquireModelUse = options.acquireModelUse;
  }
}

export function requireActiveOwner(context: EchoCaptureContext): CaptureSessionOwner {
  const owner = context.sessionOwner;
  if (owner === null || !isActive(context, owner)) throw abortOperationError();
  return owner;
}

export function assertActive(context: EchoCaptureContext, owner: CaptureSessionOwner): void {
  if (!isActive(context, owner)) throw abortOperationError();
}

export function ownsSession(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner | null,
): boolean {
  return (
    context.sessionOwner === owner && (owner === null || owner.generation === context.generation)
  );
}

export function isActive(context: EchoCaptureContext, owner: CaptureSessionOwner): boolean {
  return ownsSession(context, owner) && owner.active && !owner.signal.aborted;
}

export function captureStillCurrent(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner,
): boolean {
  return isActive(context, owner) && isCapturePhase(context.getState().phase);
}
