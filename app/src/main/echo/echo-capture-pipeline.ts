import type { HelperSessionCaptureMode } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';
import { beginExtendedTranscription, transcribe } from './echo-capture-audio';
import { EchoCaptureContext, type EchoCaptureContextOptions } from './echo-capture-context';
import { arm, beginGeneration, observeTransition, startCapture } from './echo-capture-startup';
import { performTeardown, stopCapture } from './echo-capture-teardown';
import { helperCaptureModeForPhase } from './session-phase';
import type { EchoSessionState } from './session-reducer';

export class EchoCapturePipeline {
  readonly #context: EchoCaptureContext;
  constructor(options: EchoCaptureContextOptions) {
    this.#context = new EchoCaptureContext(options);
  }
  get generation(): number {
    return this.#context.generation;
  }
  get captureId(): string | null {
    return this.#context.captureId;
  }
  get nativeCaptureLost(): boolean {
    return this.#context.nativeCaptureLost;
  }
  detachNativeCapture(): void {
    this.#context.nativeCaptureLost = true;
  }
  beginGeneration(): number {
    return beginGeneration(this.#context);
  }
  arm(settings: Readonly<Settings>): void {
    return arm(this.#context, settings);
  }
  observeTransition(previous: EchoSessionState, next: EchoSessionState): void {
    return observeTransition(this.#context, previous, next);
  }
  startCapture(): Promise<void> {
    return startCapture(this.#context);
  }
  beginExtendedTranscription(): Promise<void> {
    return beginExtendedTranscription(this.#context);
  }
  stopCapture(
    owner = this.#context.sessionOwner,
    helperMode: HelperSessionCaptureMode = helperCaptureModeForPhase(
      this.#context.getState().phase,
    ),
  ): Promise<void> {
    return stopCapture(this.#context, owner, helperMode);
  }
  transcribe(): Promise<string> {
    return transcribe(this.#context);
  }
  performTeardown(afterTimersCleared: () => void): Promise<void> {
    return performTeardown(this.#context, afterTimersCleared);
  }
}
