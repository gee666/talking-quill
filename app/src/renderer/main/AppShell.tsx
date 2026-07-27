import { lazy, Suspense, useEffect, useRef, useState, type RefObject } from 'react';
import logoDark from '../../../assets/logo-dark.png';
import logoLight from '../../../assets/logo-light.png';
import type { BootstrapData } from '../../shared/bridge/api';
import type { AppState } from '../../shared/schemas/app-state';
import type { Settings } from '../../shared/schemas/settings';
import type { WelcomeState } from '../../shared/schemas/welcome';
import { Button, Icon, Status, useTheme, type IconName } from '../design';
import { presentAppStatus } from '../status-presentation';
import { DashboardScreen } from './screens/DashboardScreen';
import { WelcomeWizard } from './welcome/WelcomeWizard';

const InfoScreen = lazy(async () => {
  const module = await import('./screens/InfoScreen');
  return { default: module.InfoScreen };
});
const SettingsScreen = lazy(async () => {
  const module = await import('./screens/SettingsScreen');
  return { default: module.SettingsScreen };
});

const screens = ['dashboard', 'settings', 'info'] as const;
type Screen = (typeof screens)[number];

const SCREEN_ICONS: Record<Screen, IconName> = {
  dashboard: 'dashboard',
  settings: 'settings',
  info: 'info',
};

export function AppShell({ bootstrap }: { readonly bootstrap: BootstrapData }) {
  const [screen, setScreen] = useState<Screen>('dashboard');
  const [state, setState] = useState<AppState>(bootstrap.state);
  const [settings, setSettings] = useState<Settings>(bootstrap.settings);
  const [welcomeReopened, setWelcomeReopened] = useState(false);
  const [maximized, setMaximized] = useState(false);
  const [theme, setTheme] = useTheme();
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
          setScreen('dashboard');
          requestAnimationFrame(() => headingRef.current?.focus());
        }}
      />
    );
  }

  const status = presentAppStatus(state.status);
  const screenContent =
    screen === 'dashboard' ? (
      <DashboardScreen
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
        onOpenWelcome={() => setWelcomeReopened(true)}
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
      <header className="brandbar">
        <div className="brandbar__drag">
          <img
            className="brandbar__logo"
            src={theme === 'light' ? logoLight : logoDark}
            alt=""
            aria-hidden="true"
          />
          <span className="brandbar__copy">
            <span className="brandbar__wordmark">Talking Quill</span>
            <span className="brandbar__tagline">Speak naturally. Write effortlessly.</span>
          </span>
        </div>
        <div className="brandbar__controls" aria-label="Window controls">
          <Button
            className="brandbar__theme"
            variant="quiet"
            aria-label={theme === 'light' ? 'Switch to dark theme' : 'Switch to light theme'}
            onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')}
          >
            <Icon name={theme === 'light' ? 'moon' : 'sun'} size={15} />
          </Button>
          <Button
            variant="quiet"
            aria-label="Minimize window"
            onClick={() => void window.talkingQuill.windowControls.minimize()}
          >
            <Icon name="minimize" size={13} />
          </Button>
          <Button
            variant="quiet"
            aria-label={maximized ? 'Restore window' : 'Maximize window'}
            aria-pressed={maximized}
            onClick={() =>
              void window.talkingQuill.windowControls.toggleMaximize().then(setMaximized)
            }
          >
            <Icon name={maximized ? 'restore' : 'maximize'} size={11} />
          </Button>
          <Button
            className="brandbar__close"
            variant="quiet"
            aria-label="Close window"
            onClick={() => void window.talkingQuill.windowControls.close()}
          >
            <Icon name="close" size={13} />
          </Button>
        </div>
      </header>
      <div className="app-frame">
        <aside className="sidebar">
          <nav aria-label="Primary">
            {screens.map((item) => (
              <button
                key={item}
                className="nav-item"
                aria-current={screen === item ? 'page' : undefined}
                onClick={() => setScreen(item)}
              >
                <Icon name={SCREEN_ICONS[item]} />
                <span>
                  {item[0]?.toUpperCase()}
                  {item.slice(1)}
                </span>
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
