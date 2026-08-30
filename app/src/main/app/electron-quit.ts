import type { App } from 'electron';

export interface BoundedElectronQuit {
  request(exitCode?: number): void;
}

type ElectronQuitTarget = Pick<App, 'exit' | 'quit'>;

/** Keeps one absolute process deadline around Electron's graceful quit path. */
export function createBoundedElectronQuit(
  target: ElectronQuitTarget,
  deadline: number,
  options: {
    readonly fallbackExitCode?: number;
    readonly onDeadline?: () => void;
  } = {},
): BoundedElectronQuit {
  let exitCode = options.fallbackExitCode ?? 1;
  let forced = false;
  const timer = setTimeout(
    () => {
      if (forced) return;
      forced = true;
      options.onDeadline?.();
      target.exit(exitCode);
    },
    Math.max(0, deadline - Date.now()),
  );

  return {
    request(requestedExitCode = 0) {
      if (forced) return;
      exitCode = requestedExitCode;
      try {
        target.quit();
      } catch {
        clearTimeout(timer);
        forced = true;
        options.onDeadline?.();
        target.exit(exitCode);
      }
    },
  };
}
