import { app } from 'electron';
import { HelperClient, activationCaptureRollbackEnabled, resolveHelperExecutable } from '../helper';
import type { ApplicationRuntime } from './application-runtime';
export function helperExecutablePath(): string | null {
  if (
    (process.platform !== 'win32' && process.platform !== 'darwin') ||
    (process.arch !== 'x64' && process.arch !== 'arm64')
  ) {
    return null;
  }
  return resolveHelperExecutable({
    packaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    appPath: app.getAppPath(),
    platform: process.platform,
  });
}

export function createHelper(
  runtime: ApplicationRuntime,
  diagnosticJournalPath: string | undefined,
): HelperClient | null {
  const platform = process.platform;
  const architecture = process.arch;
  if (
    (platform !== 'win32' && platform !== 'darwin') ||
    (architecture !== 'x64' && architecture !== 'arm64')
  ) {
    return null;
  }
  const executablePath = helperExecutablePath();
  if (executablePath === null) return null;
  return new HelperClient({
    executablePath,
    expectedHelperVersion: app.getVersion(),
    ...(diagnosticJournalPath === undefined ? {} : { diagnosticJournalPath }),
    platform,
    architecture,
    disableActivationCapture: activationCaptureRollbackEnabled(process.env),
    observeRuntimeObservability: (observability, source) => {
      void runtime.diagnostics
        ?.record('helper.runtime.snapshot', {
          component: 'helper',
          outcome: source,
          observability,
        })
        .catch(() => undefined);
    },
    observeOwnerConnectionDiagnostic: (diagnostic) =>
      runtime.diagnostics?.recordOwnerConnectionReplay(diagnostic) ?? Promise.resolve(false),
    observeProcessLifecycle: (event) => {
      void runtime.diagnostics
        ?.record(event.phase === 'started' ? 'helper.process.started' : 'helper.process.exited', {
          component: 'helper',
          outcome:
            event.phase === 'started' ? 'runtime' : event.planned === true ? 'shutdown' : 'failure',
          runtimeVersion: app.getVersion(),
          ...(event.phase === 'exited'
            ? {
                exitCode: event.exitCode ?? null,
                exitSignal: event.signal ?? null,
                planned: event.planned ?? false,
              }
            : {}),
        })
        .catch(() => undefined);
    },
  });
}
