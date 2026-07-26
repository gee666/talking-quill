import type {
  EchoSessionPhase,
  EchoSessionSnapshot,
  PiFallbackCategory,
} from '../../shared/schemas/echo-session';

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

export interface WidgetPresentation {
  readonly heading: string;
  readonly badge: 'Raw' | 'Smart';
  readonly secondary: string;
}

export function widgetPresentation(session: EchoSessionSnapshot): WidgetPresentation {
  const rawFallback =
    session.message === 'Falling back to raw' ||
    session.fallbackCategory !== null ||
    session.abortReason === 'provider-error' ||
    session.abortReason === 'timeout';
  const fallbackApplied = rawFallback && session.phase !== 'cancelled' && session.phase !== 'error';
  if (session.message === 'Cancelling insertion' && session.phase !== 'cancelled') {
    return {
      heading: 'Cancelling',
      badge: fallbackApplied || session.processingMode !== 'smart' ? 'Raw' : 'Smart',
      secondary: 'Stopping before text is inserted',
    };
  }
  return {
    heading:
      fallbackApplied && session.phase !== 'completed'
        ? 'Falling back to raw'
        : phaseHeading(session),
    badge: session.processingMode === 'smart' && !fallbackApplied ? 'Smart' : 'Raw',
    secondary: fallbackApplied ? fallbackSecondary(session) : normalSecondary(session),
  };
}

function phaseHeading(session: EchoSessionSnapshot): string {
  switch (session.phase) {
    case 'arming':
      return 'Preparing microphone';
    case 'recordingQuick':
    case 'recordingExtended':
      return 'Listening';
    case 'transcribing':
      return 'Transcribing';
    case 'processingSmart':
      return 'Processing';
    case 'inserting':
      return 'Inserting';
    case 'restoringClipboard':
      return 'Finishing';
    case 'completed':
      return session.completion === 'copied' ? 'Copied' : 'Done';
    case 'cancelled':
      return 'Cancelled';
    case 'error':
      return 'Could not complete';
    case 'idle':
      return 'Ready';
  }
}

function fallbackSecondary(session: EchoSessionSnapshot): string {
  const piFallback = piFallbackLabel(session.fallbackCategory);
  if (piFallback !== null) return piFallback;
  if (session.abortReason === 'provider-error') {
    return 'The raw transcript was used. Check the provider in Settings.';
  }
  return 'The raw transcript was used';
}

function normalSecondary(session: EchoSessionSnapshot): string {
  if (session.phase === 'cancelled') return 'Talking Quill';
  if (session.message !== null) return session.message;
  if (session.phase === 'arming') return 'Release for Quick Dictation or keep holding';
  if (session.phase === 'recordingQuick') {
    return 'Pause or press Enter to submit; press Escape to cancel';
  }
  if (session.phase === 'recordingExtended') {
    return 'Press the shortcut to stop or Escape to cancel';
  }
  return 'Talking Quill';
}
