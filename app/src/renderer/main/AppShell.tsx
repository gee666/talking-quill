import { useEffect, useRef, useState } from 'react';
import logoDark from '../../../assets/logo-dark.png';
import logoLight from '../../../assets/logo-light.png';
import type { BootstrapData } from '../../shared/bridge/api';
import type { AppState } from '../../shared/schemas/app-state';
import type { Settings } from '../../shared/schemas/settings';
import { Button, Icon, Status, useTheme, type IconName } from '../design';
import { presentAppStatus } from '../status-presentation';
import { EchoScreen } from './screens/EchoScreen';
import { InfoScreen } from './screens/InfoScreen';
import { SettingsScreen } from './screens/SettingsScreen';
import { WelcomeWizard } from './welcome/WelcomeWizard';

const screens = ['echo', 'settings', 'info'] as const;
type Screen = (typeof screens)[number];

const SCREEN_ICONS: Record<Screen, IconName> = {
  echo: 'echo',
  settings: 'settings',
  info: 'info',
};

export function AppShell({ bootstrap }: { readonly bootstrap: BootstrapData }) {
  const [screen, setScreen] = useState<Screen>('echo');
  const [state, setState] = useState<AppState>(bootstrap.state);
  const [settings, setSettings] = useState<Settings>(bootstrap.settings);
  const [welcomeReopened, setWelcomeReopened] = useState(false);
  const [welcomeCompleted, setWelcomeCompleted] = useState(
    bootstrap.settings.welcome.completedAt !== null,
  );
  const [maximized, setMaximized] = useState(false);
  const [theme, setTheme] = useTheme();
  const headingRef = useRef<HTMLHeadingElement>(null);

  useEffect(() => window.talkingQuill.app.onStateChanged(setState), []);
  useEffect(
    () =>
      window.talkingQuill.settings.onChanged((next) => {
        setSettings(next);
        if (next.welcome.completedAt !== null) setWelcomeCompleted(true);
      }),
    [],
  );
  useEffect(() => window.talkingQuill.windowControls.onMaximizedChanged(setMaximized), []);
  useEffect(() => headingRef.current?.focus(), [screen]);

  const welcomeRequired = !welcomeCompleted;
  if (welcomeRequired || welcomeReopened) {
    return (
      <WelcomeWizard
        settings={settings}
        state={state}
        platform={bootstrap.platform}
        reopened={welcomeReopened && !welcomeRequired}
        onSettingsSaved={setSettings}
        onClose={() => {
          setWelcomeReopened(false);
          requestAnimationFrame(() =>
            document.querySelector<HTMLElement>('#reopen-welcome')?.focus(),
          );
        }}
        onComplete={() => {
          setSettings((current) => ({
            ...current,
            welcome: { ...current.welcome, completedAt: Date.now(), lastStep: 6 },
          }));
          setWelcomeCompleted(true);
          setWelcomeReopened(false);
          setScreen('echo');
          requestAnimationFrame(() => headingRef.current?.focus());
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
        onSettingsSaved={setSettings}
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
          {screenContent}
        </main>
      </div>
    </div>
  );
}
