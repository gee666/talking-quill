import { useState, type RefObject } from 'react';
import { providerModelSelectionPolicy } from '../../../shared/provider-model-selection';
import type { AppState } from '../../../shared/schemas/app-state';
import type { Settings } from '../../../shared/schemas/settings';
import { Card, Status, Toast, Toggle } from '../../design';
import { formatKeyboardShortcut } from '../format-keyboard-shortcut';
import { exactAppStatusLabel, presentAppStatus } from '../../status-presentation';

const STATUS_COPY: Record<
  AppState['status'],
  { readonly heading: string; readonly introduction: string; readonly readiness: string }
> = {
  disabled: {
    heading: 'Talking Quill is off',
    introduction: 'Turn it back on whenever you want to start dictating again.',
    readiness: 'Your shortcuts stay quiet while Talking Quill is off.',
  },
  'needs-setup': {
    heading: 'Almost there',
    introduction:
      'Finish the last couple of steps below and you can start talking to any app on your computer.',
    readiness: 'A few things still need your attention.',
  },
  ready: {
    heading: 'Talking Quill is ready',
    introduction: 'Press your shortcut in any app, say what you want, and the words appear.',
    readiness: 'Everything Talking Quill needs is working.',
  },
  recording: {
    heading: 'Listening',
    introduction: 'Keep talking. Talking Quill is picking up what you say.',
    readiness: 'Your microphone is live right now.',
  },
  transcribing: {
    heading: 'Writing down your words',
    introduction: 'This happens on your own computer and only takes a moment.',
    readiness: 'Turning what you said into text.',
  },
  processing: {
    heading: 'Tidying up your text',
    introduction: 'Nearly done. Your words are being cleaned up before they go in.',
    readiness: 'Cleaning up your text.',
  },
};

export function DashboardScreen({
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
  const copy = STATUS_COPY[state.status];
  const helper = presentHelperReadiness(state.helper.status);
  const setupRequirement = missingSetupRequirement(state, platform);
  const smartProviderId = settings.smartProcessing.selectedProviderId;
  const smartProviderConfig = settings.smartProcessing.providers[smartProviderId];
  const smartProviderReadiness =
    providerModelSelectionPolicy(smartProviderId) === 'provider-managed'
      ? `${smartProviderId} uses its currently loaded model`
      : smartProviderConfig?.modelId
        ? `${smartProviderId} model selected`
        : 'Pick a model to finish setup';

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
          <p className="eyebrow">Dashboard</p>
          <h1 ref={headingRef} tabIndex={-1}>
            {copy.heading}
          </h1>
          <p>{setupRequirement ?? copy.introduction}</p>
        </div>
        <Status tone={status.tone}>{exactAppStatusLabel(state, platform)}</Status>
      </header>
      <div className="screen__grid">
        <Card
          title="Dictation"
          description="Switch Talking Quill on when you want to dictate. With Raw dictation your voice never leaves your computer: no account, nothing tracked, nothing uploaded."
        >
          <Toggle
            checked={state.enabled}
            disabled={savingEnabled}
            onChange={(event) => void updateEnabled(event.currentTarget.checked)}
            label={state.enabled ? 'Talking Quill enabled' : 'Talking Quill disabled'}
            hint="While it is on, your shortcut works in any app on your computer."
          />
        </Card>
        {enabledError === null ? null : (
          <Toast tone="error" message={enabledError} onDismiss={() => setEnabledError(null)} />
        )}
        <Card title="How you are set up" description="What Talking Quill will use when you speak.">
          <h3 className="subhead">Your shortcuts</h3>
          <dl className="details-list" aria-label="Your shortcuts">
            {settings.dictationProfiles.map((profile) => (
              <div key={profile.id}>
                <dt>{profile.name}</dt>
                <dd>
                  {formatKeyboardShortcut(profile.shortcut, platform)} ·{' '}
                  {profile.processingMode === 'raw' ? 'Raw' : 'Smart'}
                </dd>
              </div>
            ))}
          </dl>
          <h3 className="subhead">Everything else</h3>
          <dl className="details-list" aria-label="Everything else">
            <div>
              <dt>Microphone</dt>
              <dd>{settings.recording.preferredMicrophoneId ?? 'System default'}</dd>
            </div>
            <div>
              <dt>Speech model</dt>
              <dd>
                {state.modelReady
                  ? 'Model available'
                  : 'Install or repair it in Settings > Speech model'}
              </dd>
            </div>
            <div>
              <dt>AI clean-up</dt>
              <dd>{smartProviderReadiness}</dd>
            </div>
          </dl>
        </Card>
        <Card title="Try it here" description="Click in the box, then use your shortcut and talk.">
          <label className="try-dictation">
            <span>Dictation test area</span>
            <textarea
              className="me-field__control"
              rows={3}
              placeholder="Your inserted dictation will appear here…"
            />
          </label>
        </Card>
        <Card title="What is ready" description={copy.readiness}>
          <div className="group readiness-group">
            <div className="readiness-row">
              <span>Talking Quill itself</span>
              <Status tone="success">Available</Status>
            </div>
            <div className="readiness-row">
              <span>Typing helper</span>
              <Status tone={helper.tone}>{helper.label}</Status>
            </div>
            <p className="body-copy readiness-note">{helperReadinessDetail(state, platform)}</p>
            <div className="readiness-row">
              <span>Speech model</span>
              <Status tone={state.modelReady ? 'success' : 'warning'}>
                {state.modelReady ? 'Available' : 'Model missing'}
              </Status>
            </div>
          </div>
        </Card>
      </div>
    </div>
  );
}

function missingSetupRequirement(state: AppState, platform: string): string | null {
  if (state.status !== 'needs-setup') return null;
  if (!state.modelReady) {
    return 'The selected speech model is unavailable. Open Settings > Speech model and install or repair it.';
  }
  return helperReadinessDetail(state, platform);
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
      return { label: 'Needs permission', tone: 'warning' };
    case 'incompatible':
      return { label: 'Needs an update', tone: 'error' };
    case 'unavailable':
      return { label: 'Not working', tone: 'error' };
    case 'stopped':
      return { label: 'Stopped', tone: 'neutral' };
  }
}

function helperReadinessDetail(state: AppState, platform: string): string {
  const ownerName = platform === 'darwin' ? 'keyboard service' : 'local keyboard owner';
  switch (state.helper.reason) {
    case 'input-monitoring-required':
      return 'Open System Settings on your Mac, allow Input Monitoring for Talking Quill, then restart the app.';
    case 'accessibility-required':
    case 'event-post-required':
      return 'Open System Settings on your Mac and allow Accessibility, so Talking Quill can type for you.';
    case 'binary-missing':
      return 'Part of Talking Quill is missing. Reinstalling the app will put it back.';
    case 'protocol-mismatch':
      return 'This part of Talking Quill does not match the rest of the app. Reinstalling will fix it.';
    case 'crash-loop':
      return `The ${ownerName} keeps stopping. Talking Quill will try to start it again automatically. Reinstall the app if it still cannot start.`;
    case 'capture-disabled':
      return 'Global shortcuts are safely disabled in this build.';
    case 'owner-missing':
      return `The ${ownerName} could not start. Talking Quill will try again automatically. Reinstall the app if it remains unavailable.`;
    case 'owner-auth-failed':
    case 'owner-security-fault':
      return `Talking Quill could not verify its ${ownerName}. Repair or reinstall the app before using shortcuts.`;
    case 'owner-busy':
      return `Another Talking Quill controller is using the ${ownerName}. Close it and Talking Quill will try again automatically.`;
    case 'owner-draining':
      return `Release all shortcut keys while the ${ownerName} finishes safely. Talking Quill will reconnect automatically.`;
    case 'owner-maintenance':
      return `The ${ownerName} is being updated. Shortcuts will return automatically when maintenance finishes.`;
    case 'owner-incompatible':
      return `The ${ownerName} needs the same Talking Quill version. Update or repair the app.`;
    case 'owner-rollback':
      return 'Keyboard shortcuts are disabled by the recovery safety latch.';
    case 'owner-degraded':
      return `The ${ownerName} lost its safe connection. Talking Quill will restart it automatically.`;
    case 'owner-indeterminate':
      return `The ${ownerName} cannot prove a safe state. Repair or reinstall the app before using shortcuts.`;
    case null:
      return state.helper.status === 'ready'
        ? 'Your shortcut works everywhere, and Talking Quill can type into whichever app you are using.'
        : `Checking the ${ownerName}.`;
    default:
      return `The ${ownerName} could not start. Talking Quill will try again automatically. Reinstall the app if it remains unavailable.`;
  }
}
