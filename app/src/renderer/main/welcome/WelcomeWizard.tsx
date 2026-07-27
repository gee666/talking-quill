/* eslint-disable jsx-a11y/no-noninteractive-tabindex -- the constrained setup region must receive PageDown/End keyboard focus */
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react';
import logoDark from '../../../../assets/logo-dark.png';
import logoLight from '../../../../assets/logo-light.png';
import { ECHO_HOLD_THRESHOLD_MS } from '../../../shared/constants/echo-session';
import type { AppState } from '../../../shared/schemas/app-state';
import { GENERAL_PROFILE_ID } from '../../../shared/schemas/dictation-profiles';
import type { Settings } from '../../../shared/schemas/settings';
import type { WelcomeState, WelcomeStep } from '../../../shared/schemas/welcome';
import { Button, Card, Status, useTheme } from '../../design';
import { RecordingSection } from '../settings/RecordingSection';
import { ModelSetup } from '../setup/ModelSetup';
import { formatKeyboardShortcutWithTrigger } from '../format-keyboard-shortcut';
import { publicErrorMessage } from '../public-error';

const SmartProcessingSection = lazy(async () => {
  const module = await import('../SmartProcessingSection');
  return { default: module.SmartProcessingSection };
});

const STEP_NAMES = ['Welcome', 'Microphone', 'Local model', 'Smart processing', 'Ready'] as const;

export function WelcomeWizard({
  settings,
  state,
  platform,
  reopened,
  onSettingsSaved,
  onComplete,
  onClose,
}: {
  readonly settings: Settings;
  readonly state: AppState;
  readonly platform: string;
  readonly reopened: boolean;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly onComplete: (state: WelcomeState) => void;
  readonly onClose: () => void;
}) {
  const authoritativeRevision = settings.welcome.revision ?? 0;
  const [optimisticStep, setOptimisticStep] = useState(() => ({
    step: settings.welcome.lastStep,
    source: settings.welcome,
  }));
  const step =
    optimisticStep.source === settings.welcome ? optimisticStep.step : settings.welcome.lastStep;
  const [saving, setSaving] = useState(false);
  const [showSmartProcessing, setShowSmartProcessing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contentScrollable, setContentScrollable] = useState(false);
  const [theme] = useTheme();
  const heading = useRef<HTMLHeadingElement>(null);
  const content = useRef<HTMLElement>(null);
  const latestWelcomeRevision = useRef(settings.welcome.revision ?? 0);
  useLayoutEffect(() => {
    latestWelcomeRevision.current = authoritativeRevision;
  }, [authoritativeRevision]);
  useEffect(() => heading.current?.focus(), [step]);
  useEffect(() => {
    const element = content.current;
    if (element === null) return;
    const measure = () => setContentScrollable(element.scrollHeight > element.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const child of element.children) observer.observe(child);
    return () => observer.disconnect();
  }, [step]);

  const move = async (next: WelcomeStep) => {
    setSaving(true);
    setError(null);
    try {
      const saved = await window.talkingQuill.welcome.setStep(next);
      if ((saved.revision ?? 0) >= latestWelcomeRevision.current) {
        setOptimisticStep({ step: saved.lastStep, source: settings.welcome });
      }
    } catch (cause: unknown) {
      setError(publicErrorMessage(cause, 'Welcome progress could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  const skipSmartProcessing = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await window.talkingQuill.profiles.update(GENERAL_PROFILE_ID, {
        processingMode: 'raw',
      });
      onSettingsSaved(saved);
      const welcome = await window.talkingQuill.welcome.setStep(5);
      if ((welcome.revision ?? 0) >= latestWelcomeRevision.current) {
        setOptimisticStep({ step: welcome.lastStep, source: settings.welcome });
      }
    } catch (cause: unknown) {
      setError(publicErrorMessage(cause, 'Raw processing preference could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  const complete = async () => {
    setSaving(true);
    setError(null);
    try {
      const welcome = await window.talkingQuill.welcome.complete();
      onComplete(welcome);
    } catch (cause: unknown) {
      setError(publicErrorMessage(cause, 'Setup completion could not be saved.'));
    } finally {
      setSaving(false);
    }
  };

  return (
    <main className="welcome" aria-labelledby="welcome-heading">
      <div className="welcome__brand">
        <img
          className="welcome__brand-logo"
          src={theme === 'light' ? logoLight : logoDark}
          alt=""
          aria-hidden="true"
        />
        <span className="welcome__brand-copy">
          <span className="welcome__brand-wordmark">Talking Quill</span>
          <span className="welcome__brand-tagline">Speak naturally. Write effortlessly.</span>
        </span>
      </div>
      <header className="welcome__header">
        <h1 id="welcome-heading" ref={heading} tabIndex={-1} className="welcome__title">
          {STEP_NAMES[step - 1]}
        </h1>
        <div className="welcome__header-meta">
          <p className="eyebrow welcome__counter">Setup · Step {step} of 5</p>
          {reopened ? (
            <Button variant="quiet" onClick={onClose}>
              Exit Welcome
            </Button>
          ) : null}
        </div>
      </header>
      <ol className="welcome__progress" aria-label="Setup progress">
        {STEP_NAMES.map((name, index) => {
          const position = index + 1;
          const progressState =
            position < step ? 'complete' : position === step ? 'current' : 'upcoming';
          return (
            <li
              key={name}
              data-state={progressState}
              aria-current={progressState === 'current' ? 'step' : undefined}
            >
              <span className="welcome__progress-marker" aria-hidden="true" />
              <span className="welcome__progress-label">{name}</span>
            </li>
          );
        })}
      </ol>
      <section
        ref={content}
        className="welcome__content"
        aria-label={`${STEP_NAMES[step - 1] ?? 'Welcome'} setup controls`}
        aria-busy={saving}
        inert={saving ? true : undefined}
        tabIndex={contentScrollable ? 0 : undefined}
      >
        {renderStep(step)}
      </section>
      {error === null ? null : (
        <p role="alert" className="operation-message operation-message--error">
          {error}
        </p>
      )}
      <footer className="welcome__actions">
        <Button
          variant="secondary"
          disabled={saving || step === 1}
          onClick={() => void move((step - 1) as WelcomeStep)}
        >
          Back
        </Button>
        {step === 4 ? (
          <>
            <Button variant="quiet" busy={saving} onClick={() => void skipSmartProcessing()}>
              Skip Smart processing
            </Button>
            <Button busy={saving} onClick={() => void move(5)}>
              Continue
            </Button>
          </>
        ) : step < 5 ? (
          <Button busy={saving} onClick={() => void move((step + 1) as WelcomeStep)}>
            Continue
          </Button>
        ) : (
          <Button busy={saving} onClick={() => void complete()}>
            Start using Talking Quill
          </Button>
        )}
      </footer>
    </main>
  );

  function renderStep(current: WelcomeStep) {
    if (current === 1)
      return (
        <Card title="Private dictation, ready anywhere">
          <p className="body-copy">
            Talking Quill turns speech into text locally and inserts it into the application you are
            using.
          </p>
          <p>
            <strong>Free to use — no account, no usage limits.</strong>
          </p>
          <p className="body-copy">
            Optional cloud providers may charge for their own service. Raw transcription never sends
            audio or text to a provider.
          </p>
        </Card>
      );
    if (current === 2) return <RecordingSection settings={settings} platform={platform} />;
    if (current === 3)
      return (
        <Card
          title="Download local Whisper"
          description="The model is downloaded once and then works offline."
        >
          <ModelSetup settings={settings} onSettingsSaved={onSettingsSaved} />
          <Status tone={state.modelReady ? 'success' : 'warning'}>
            {state.modelReady ? 'Selected model ready' : 'Download required'}
          </Status>
        </Card>
      );
    if (current === 4)
      return (
        <>
          <Card title="Optional Smart processing">
            <p className="body-copy">
              Skip this step for fully local Raw transcription. Ollama is highlighted as a local
              option; cloud providers may charge your account.
            </p>
          </Card>
          {showSmartProcessing ? (
            <Suspense fallback={<p aria-live="polite">Loading Smart processing…</p>}>
              <SmartProcessingSection settings={settings} onSettingsSaved={onSettingsSaved} />
            </Suspense>
          ) : (
            <Button variant="secondary" onClick={() => setShowSmartProcessing(true)}>
              Configure Smart processing
            </Button>
          )}
        </>
      );
    return (
      <>
        <Card title="Talking Quill is ready">
          <p>Your configured dictation profiles are ready:</p>
          <ul aria-label="Configured dictation profiles">
            {settings.dictationProfiles.map((profile) => (
              <li key={profile.id}>
                <strong>{profile.name}</strong>:{' '}
                {formatKeyboardShortcutWithTrigger(profile.shortcut, platform)} —{' '}
                {profile.processingMode === 'raw' ? 'Raw' : 'Smart'}
                {' processing'}
              </li>
            ))}
          </ul>
          <ul>
            <li>
              Quick Dictation starts when you release the shortcut&apos;s final letter key before{' '}
              {String(ECHO_HOLD_THRESHOLD_MS)} ms and submits after trailing silence.
            </li>
            <li>
              Extended Dictation starts when you keep the shortcut&apos;s final letter key down for{' '}
              {String(ECHO_HOLD_THRESHOLD_MS)} ms and keeps recording through silence until you
              press Enter, press the full shortcut chord again, or use Stop.
            </li>
            <li>Escape cancels either mode.</li>
          </ul>
          <p>Shortcuts can be changed anytime in Settings under Dictation profiles.</p>
          <Status
            tone={state.modelReady && state.helper.status === 'ready' ? 'success' : 'warning'}
          >
            {state.modelReady && state.helper.status === 'ready'
              ? 'Ready for first dictation'
              : 'Setup can be revisited from Info'}
          </Status>
        </Card>
        {platform === 'darwin' ? (
          <Card title="macOS permissions">
            <p className="body-copy">
              Allow Accessibility and Input Monitoring so global shortcuts and text insertion work.
            </p>
            <Status tone={state.helper.status === 'ready' ? 'success' : 'warning'}>
              {state.helper.status === 'ready' ? 'Permissions ready' : 'Permission setup required'}
            </Status>
            <div className="provider-actions">
              <Button
                variant="secondary"
                onClick={() =>
                  void window.talkingQuill.info.openPermissionSettings('accessibility')
                }
              >
                Open Accessibility settings
              </Button>
              <Button
                variant="secondary"
                onClick={() =>
                  void window.talkingQuill.info.openPermissionSettings('input-monitoring')
                }
              >
                Open Input Monitoring settings
              </Button>
            </div>
          </Card>
        ) : null}
      </>
    );
  }
}
