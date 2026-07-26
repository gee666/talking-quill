import type { EchoSessionSnapshot } from '../../shared/schemas/echo-session';

export function isCapturePhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'arming' || phase === 'recordingQuick' || phase === 'recordingExtended';
}

export function isTerminalPhase(phase: EchoSessionSnapshot['phase']): boolean {
  return phase === 'completed' || phase === 'cancelled' || phase === 'error';
}
