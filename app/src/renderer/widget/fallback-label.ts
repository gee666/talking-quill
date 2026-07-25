import type { EchoSessionPhase, PiFallbackCategory } from '../../shared/schemas/echo-session';

export function isWidgetPointerCancelable(phase: EchoSessionPhase): boolean {
  return (
    phase === 'arming' ||
    phase === 'recordingQuick' ||
    phase === 'recordingExtended' ||
    phase === 'transcribing' ||
    phase === 'processingSmart'
  );
}

export function piFallbackLabel(category: PiFallbackCategory | null): string | null {
  switch (category) {
    case 'pi-unavailable':
      return 'The raw transcript was used. Pi is unavailable; check its installation in Settings.';
    case 'pi-authentication-failed':
      return 'The raw transcript was used. Sign in to the selected provider in Pi, then retry.';
    case 'pi-model-not-found':
    case 'pi-no-models':
      return 'The raw transcript was used. Refresh Pi models in Settings and select one.';
    case 'pi-timeout':
      return 'The raw transcript was used because Pi timed out.';
    case 'pi-invalid-response':
    case 'pi-remote-failure':
      return 'The raw transcript was used because Pi failed safely.';
    case null:
      return null;
  }
}
