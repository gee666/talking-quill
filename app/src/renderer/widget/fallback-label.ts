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
    phase === 'processingSmart' ||
    phase === 'inserting'
  );
}

export function widgetKeyboardGuidance(session: EchoSessionSnapshot): string {
  if (
    session.phase === 'arming' ||
    session.phase === 'recordingQuick' ||
    session.phase === 'recordingExtended'
  ) {
    return session.dictationMode === 'extended'
      ? 'Press Enter or your dictation shortcut again to finish, Escape to cancel, or click Stop or Cancel here.'
      : 'Press Enter or your dictation shortcut again to finish, Escape to cancel, or click Cancel here.';
  }
  if (isWidgetPointerCancelable(session.phase)) {
    return 'Press Escape or click Cancel here to stop before text is inserted.';
  }
  return 'Text has already been inserted or this dictation is finished.';
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
  readonly badge: 'Raw' | 'Smart' | 'Error';
  readonly secondary: string;
}

export function widgetPresentation(session: EchoSessionSnapshot): WidgetPresentation {
  if (session.phase === 'error') {
    return {
      heading: 'Could not complete',
      badge: 'Error',
      secondary: session.message ?? 'Talking Quill encountered an error.',
    };
  }
  const rawFallback =
    session.message === 'Falling back to raw' ||
    session.fallbackCategory !== null ||
    session.abortReason === 'provider-error' ||
    session.abortReason === 'timeout';
  const fallbackApplied = rawFallback && session.phase !== 'cancelled';
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
  if (session.phase === 'arming') {
    return 'Let go for a quick note, or keep holding for a longer one';
  }
  if (session.phase === 'recordingQuick') {
    return 'Press Enter or your shortcut again when you are done; Escape cancels';
  }
  if (session.phase === 'recordingExtended') {
    return 'Press Enter or your shortcut again to finish; Escape cancels';
  }
  return 'Talking Quill';
}
