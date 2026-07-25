import { lazy, Suspense, useEffect, useRef, useState, type RefObject } from 'react';
import appIcon from '../../../assets/app-icon.png';
import type { BootstrapData } from '../../shared/bridge/api';
import type { AppState } from '../../shared/schemas/app-state';
import type { Settings } from '../../shared/schemas/settings';
import type { WelcomeState } from '../../shared/schemas/welcome';
import { Button, Status } from '../design';
import { presentAppStatus } from '../status-presentation';
import { EchoScreen } from './screens/EchoScreen';
import { WelcomeWizard } from './welcome/WelcomeWizard';

const InfoScreen = lazy(async () => {
  const module = await import('./screens/InfoScreen');
  return { default: module.InfoScreen };
});
const SettingsScreen = lazy(async () => {
  const module = await import('./screens/SettingsScreen');
  return { default: module.SettingsScreen };
});

const screens = ['echo', 'settings', 'info'] as const;
type Screen = (typeof screens)[number];

export function AppShell({ bootstrap }: { readonly bootstrap: BootstrapData }) {
  const [screen, setScreen] = useState<Screen>('echo');
  const [state, setState] = useState<AppState>(bootstrap.state);
  const [settings, setSettings] = useState<Settings>(bootstrap.settings);
  const [welcomeReopened, setWelcomeReopened] = useState(false);
  const [maximized, setMaximized] = useState(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const welcomeRequired = settings.welcome.completedAt === null;

  useEffect(() => window.talkingQuill.app.onStateChanged(setState), []);
  useEffect(
    () =>
      window.talkingQuill.settings.onChanged((next) =>
        setSettings((current) => mergeSettingsSnapshot(current, next)),
      ),
    [],
  );
  useEffect(() => window.talkingQuill.windowControls.onMaximizedChanged(setMaximized), []);
  if (welcomeRequired || welcomeReopened) {
    return (
      <WelcomeWizard
        settings={settings}
        state={state}
        platform={bootstrap.platform}
        reopened={welcomeReopened && !welcomeRequired}
        onSettingsSaved={(next) => setSettings((current) => mergeSettingsSnapshot(current, next))}
        onClose={() => {
          setWelcomeReopened(false);
          requestAnimationFrame(() =>
            document.querySelector<HTMLElement>('#reopen-welcome')?.focus(),
          );
        }}
        onComplete={(welcome) => {
          setSettings((current) => mergeWelcomeState(current, welcome));
          setWelcomeReopened(false);
          setScreen('echo');
        }}
      />
    );
  }

  const status = presentAppStatus(state.status);
  const screenContent =
    screen === 'echo' ? (
      <EchoScreen
        headingRef={headingRef}
        state={state}
        settings={settings}
        platform={bootstrap.platform}
      />
    ) : screen === 'settings' ? (
      <SettingsScreen
        headingRef={headingRef}
        settings={settings}
        platform={bootstrap.platform}
        onSettingsSaved={(next) => setSettings((current) => mergeSettingsSnapshot(current, next))}
      />
    ) : (
      <InfoScreen
        headingRef={headingRef}
        bootstrap={{ ...bootstrap, state, settings }}
        onOpenWelcome={() => setWelcomeReopened(true)}
      />
    );

  return (
    <div className="app-shell">
      <header className="titlebar">
        <div className="titlebar__drag">
          <img className="titlebar__mark" src={appIcon} alt="" aria-hidden="true" />
          <span>Talking Quill</span>
        </div>
        <div className="titlebar__controls" aria-label="Window controls">
          <Button
            variant="quiet"
            aria-label="Minimize window"
            onClick={() => void window.talkingQuill.windowControls.minimize()}
          >
            —
          </Button>
          <Button
            variant="quiet"
            aria-label={maximized ? 'Restore window' : 'Maximize window'}
            aria-pressed={maximized}
            onClick={() =>
              void window.talkingQuill.windowControls.toggleMaximize().then(setMaximized)
            }
          >
            {maximized ? '❐' : '□'}
          </Button>
          <Button
            variant="quiet"
            aria-label="Close window"
            onClick={() => void window.talkingQuill.windowControls.close()}
          >
            ×
          </Button>
        </div>
      </header>
      <div className="app-frame">
        <aside className="sidebar">
          <div className="sidebar__brand">
            <img className="sidebar__logo" src={appIcon} alt="" aria-hidden="true" />
            <div>
              <strong>Talking Quill</strong>
              <span>Local dictation</span>
            </div>
          </div>
          <nav aria-label="Primary">
            {screens.map((item) => (
              <button
                key={item}
                className="nav-item"
                aria-current={screen === item ? 'page' : undefined}
                onClick={() => setScreen(item)}
              >
                <span aria-hidden="true">
                  {item === 'echo' ? '◉' : item === 'settings' ? '⚙' : 'ⓘ'}
                </span>
                {item[0]?.toUpperCase()}
                {item.slice(1)}
              </button>
            ))}
          </nav>
          <Status tone={status.tone} live>
            {status.label}
          </Status>
        </aside>
        <main className="content" id="main-content">
          <Suspense fallback={<p aria-live="polite">Loading screen…</p>}>
            {screenContent}
            <ScreenHeadingFocus headingRef={headingRef} screen={screen} />
          </Suspense>
        </main>
      </div>
    </div>
  );
}

function ScreenHeadingFocus({
  headingRef,
  screen,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
  readonly screen: Screen;
}) {
  useEffect(() => headingRef.current?.focus(), [headingRef, screen]);
  return null;
}

function mergeSettingsSnapshot(current: Settings, next: Settings): Settings {
  if ((current.welcome.revision ?? 0) <= (next.welcome.revision ?? 0)) return next;
  return { ...next, welcome: current.welcome };
}

function mergeWelcomeState(current: Settings, state: WelcomeState): Settings {
  const welcome: Settings['welcome'] = {
    completedAt: state.completedAt,
    lastStep: state.lastStep,
    microphoneTested: state.microphoneTested,
    activationTested: state.activationTested,
    microphoneEvidence: state.microphoneEvidence,
    activationEvidence: state.activationEvidence,
    modelEvidence: state.modelEvidence,
    revision: state.revision,
  };
  const currentRevision = current.welcome.revision ?? 0;
  const nextRevision = welcome.revision ?? 0;
  if (
    currentRevision > nextRevision ||
    (currentRevision === nextRevision &&
      current.welcome.completedAt === welcome.completedAt &&
      current.welcome.lastStep === welcome.lastStep)
  ) {
    return current;
  }
  return { ...current, welcome };
}
