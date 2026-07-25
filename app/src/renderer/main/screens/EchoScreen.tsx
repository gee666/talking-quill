import { useState, type RefObject } from 'react';
import type { AppState } from '../../../shared/schemas/app-state';
import type { Settings } from '../../../shared/schemas/settings';
import { Card, Status, Toast, Toggle } from '../../design';
import { PastEchoes } from '../history/PastEchoes';
import { presentAppStatus } from '../../status-presentation';

const ECHO_STATUS_COPY: Record<
  AppState['status'],
  { readonly heading: string; readonly introduction: string; readonly readiness: string }
> = {
  disabled: {
    heading: 'Talking Quill is disabled',
    introduction: 'Enable Talking Quill when you are ready to start dictating.',
    readiness: 'Activation is disabled and no shortcut gestures will start dictation.',
  },
  'needs-setup': {
    heading: 'Dictation needs local setup',
    introduction:
      'Install the selected local transcription model and grant the requested permissions to begin dictating.',
    readiness: 'Local transcription still needs setup before dictation can start.',
  },
  ready: {
    heading: 'Talking Quill is ready',
    introduction: 'Local dictation services are configured and ready on this device.',
    readiness: 'The local dictation foundation is ready.',
  },
  recording: {
    heading: 'Listening for speech',
    introduction: 'Talking Quill is capturing speech for local transcription.',
    readiness: 'The local capture service is active.',
  },
  transcribing: {
    heading: 'Transcribing speech',
    introduction: 'Talking Quill is converting captured speech into text on this device.',
    readiness: 'Local transcription is in progress.',
  },
  processing: {
    heading: 'Preparing your text',
    introduction: 'Talking Quill is processing captured speech on this device.',
    readiness: 'Local text processing is in progress.',
  },
};

export function EchoScreen({
  headingRef,
  state,
  settings,
  platform,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
  readonly state: AppState;
  readonly settings: Settings;
  readonly platform: string;
}) {
  const [savingEnabled, setSavingEnabled] = useState(false);
  const [enabledError, setEnabledError] = useState<string | null>(null);
  const status = presentAppStatus(state.status);
  const copy = ECHO_STATUS_COPY[state.status];
  const helper = presentHelperReadiness(state.helper.status);

  const updateEnabled = async (enabled: boolean) => {
    setSavingEnabled(true);
    setEnabledError(null);
    try {
      await window.talkingQuill.app.setEnabled(enabled);
    } catch {
      setEnabledError('Talking Quill could not save the enabled setting.');
    } finally {
      setSavingEnabled(false);
    }
  };

  return (
    <div className="screen">
      <header className="screen__header">
        <div>
          <p className="eyebrow">Echo</p>
          <h1 ref={headingRef} tabIndex={-1}>
            {copy.heading}
          </h1>
          <p>{copy.introduction}</p>
        </div>
        <Status tone={status.tone}>{status.label}</Status>
      </header>
      <div className="screen__grid">
        <Card title="Application" description="Store whether Talking Quill should be enabled.">
          <Toggle
            checked={state.enabled}
            disabled={savingEnabled}
            onChange={(event) => void updateEnabled(event.currentTarget.checked)}
            label={state.enabled ? 'Talking Quill enabled' : 'Talking Quill disabled'}
            hint="Use the configured Alt or Option shortcut from any application."
          />
        </Card>
        {enabledError === null ? null : (
          <Toast tone="error" message={enabledError} onDismiss={() => setEnabledError(null)} />
        )}
        <Card
          title="Privacy by design"
          description="Local transcription is the default foundation."
        >
          <p className="body-copy">
            Raw transcription keeps audio on this device. Talking Quill includes no telemetry and
            requires no account.
          </p>
        </Card>
        <Card title="Current setup" description="Your active dictation configuration.">
          <dl className="details-list">
            <div>
              <dt>Profiles</dt>
              <dd>
                {settings.dictationProfiles
                  .map(
                    (profile) =>
                      `${profile.name}: ${platform === 'darwin' ? 'Option' : 'Alt'} + ${profile.shift ? 'Shift + ' : ''}${profile.activationKey} (${profile.processingMode === 'raw' ? 'Raw' : 'Smart'})`,
                  )
                  .join(' · ')}
              </dd>
            </div>
            <div>
              <dt>Microphone</dt>
              <dd>{settings.recording.preferredMicrophoneId ?? 'System default'}</dd>
            </div>
            <div>
              <dt>Transcription model</dt>
              <dd>{state.modelReady ? 'Model available' : 'Needs setup'}</dd>
            </div>
            <div>
              <dt>Smart provider</dt>
              <dd>
                {settings.smartProcessing.providers[settings.smartProcessing.selectedProviderId]
                  ?.modelId
                  ? `${settings.smartProcessing.selectedProviderId} configured`
                  : 'Needs provider model setup'}
              </dd>
            </div>
          </dl>
        </Card>
        <Card
          title="Try it here"
          description="Focus this field, then use your activation shortcut."
        >
          <label className="try-echo">
            <span>Dictation test area</span>
            <textarea
              className="me-field__control"
              rows={3}
              placeholder="Your inserted dictation will appear here…"
            />
          </label>
          <p className="body-copy">
            Release quickly for Quick Dictation; hold for 600 ms for Extended Dictation. Press
            Enter, press the shortcut again, or use the widget Stop button to submit. Press Escape
            to cancel. Each profile shortcut uses its configured Raw or Smart mode.
          </p>
        </Card>
        <Card title="Current readiness" description={copy.readiness}>
          <div className="group readiness-group">
            <div className="readiness-row">
              <span>Desktop shell and local settings</span>
              <Status tone="success">Available</Status>
            </div>
            <div className="readiness-row">
              <span>Native keyboard and insertion helper</span>
              <Status tone={helper.tone}>{helper.label}</Status>
            </div>
            <p className="body-copy readiness-note">{helperReadinessDetail(state)}</p>
            <div className="readiness-row">
              <span>Local transcription model</span>
              <Status tone={status.tone}>{status.label}</Status>
            </div>
          </div>
        </Card>
        <PastEchoes />
      </div>
    </div>
  );
}

function presentHelperReadiness(status: AppState['helper']['status']): {
  readonly label: string;
  readonly tone: 'neutral' | 'info' | 'success' | 'warning' | 'error';
} {
  switch (status) {
    case 'starting':
      return { label: 'Checking', tone: 'info' };
    case 'ready':
      return { label: 'Available', tone: 'success' };
    case 'permission-required':
      return { label: 'Permission needed', tone: 'warning' };
    case 'incompatible':
      return { label: 'Update needed', tone: 'error' };
    case 'unavailable':
      return { label: 'Unavailable', tone: 'error' };
    case 'stopped':
      return { label: 'Stopped', tone: 'neutral' };
  }
}

function helperReadinessDetail(state: AppState): string {
  switch (state.helper.reason) {
    case 'input-monitoring-required':
      return 'Allow Input Monitoring in macOS System Settings, then restart Talking Quill.';
    case 'accessibility-required':
    case 'event-post-required':
      return 'Allow Accessibility in macOS System Settings so Talking Quill can insert text.';
    case 'binary-missing':
      return 'The native helper binary is missing. Reinstall Talking Quill to repair this component.';
    case 'protocol-mismatch':
      return 'The native helper does not match this application version. Reinstall Talking Quill.';
    case 'crash-loop':
      return 'The native helper stopped repeatedly. Restart Talking Quill or reinstall the application.';
    case null:
      return state.helper.status === 'ready'
        ? 'Global activation, session controls, front-app inspection, and paste dispatch are available.'
        : 'Checking the bundled native helper.';
    default:
      return 'The native helper could not start. Restart Talking Quill or reinstall the application.';
  }
}
