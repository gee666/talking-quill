import { useEffect, useState } from 'react';
import type { BootstrapData } from '../../shared/bridge/api';
import { EmptyState } from '../design';
import { AppShell } from './AppShell';

export function App() {
  const [bootstrap, setBootstrap] = useState<BootstrapData | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void window.talkingQuill.app
      .getBootstrap()
      .then((data) => {
        if (active) setBootstrap(data);
      })
      .catch(() => {
        if (active)
          setError('Talking Quill could not load its local settings. Restart the application.');
      });
    return () => {
      active = false;
    };
  }, []);

  if (error !== null) {
    return (
      <main className="startup-state">
        <EmptyState title="Unable to start" description={error} />
      </main>
    );
  }
  if (bootstrap === null) {
    return (
      <main className="startup-state" aria-busy="true">
        <p>Loading Talking Quill…</p>
      </main>
    );
  }
  return <AppShell bootstrap={bootstrap} />;
}
