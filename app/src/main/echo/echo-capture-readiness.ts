import type { EchoCaptureContext } from './echo-capture-context';
import { isCapturePhase } from './session-phase';

export function announceCaptureReady(context: EchoCaptureContext): void {
  const state = context.getState();
  if (
    context.readyCuePlayed ||
    !state.captureReady ||
    !state.audioReady ||
    !isCapturePhase(state.phase)
  )
    return;
  context.readyCuePlayed = true;
  context.playSound();
}
