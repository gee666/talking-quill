import type { HelperSessionCaptureMode } from '../../shared/helper/protocol';
import type { EchoSessionSnapshot } from '../../shared/schemas/echo-session';

export function isCapturePhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'arming' || phase === 'recordingQuick' || phase === 'recordingExtended';
}

export function helperCaptureModeForPhase(
  phase: EchoSessionSnapshot['phase'],
): HelperSessionCaptureMode {
  if (isCapturePhase(phase)) return 'recording';
  if (phase === 'transcribing' || phase === 'processingSmart' || phase === 'inserting') {
    return 'cancel-only';
  }
  return 'off';
}

export function isTerminalPhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'completed' || phase === 'cancelled' || phase === 'error';
}
