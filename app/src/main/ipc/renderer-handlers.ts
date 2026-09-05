import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';

type RendererHandlerDependencies = Pick<
  HandlerDependencies,
  'echo' | 'packagedMediaReady' | 'recording' | 'windows'
>;
type RendererHandlers = Pick<
  InvokeHandlerMap,
  | 'activation-test:start'
  | 'activation-test:stop'
  | 'shortcut-capture:start'
  | 'shortcut-capture:stop'
  | 'window:minimize'
  | 'window:toggle-maximize'
  | 'window:close'
  | 'widget:ready'
  | 'widget:stop'
  | 'widget:cancel'
  | 'widget:set-interactive'
  | 'capture:ready'
  | 'recording:get-devices'
  | 'recording:start-test'
  | 'recording:stop-test'
  | 'recording:open-microphone-settings'
>;

export function createRendererHandlers(
  dependencies: RendererHandlerDependencies,
): RendererHandlers {
  return {
    'activation-test:start': (_request, context) =>
      dependencies.echo.startActivationTest(context.webContentsId, context.onDestroyed),
    'activation-test:stop': (_request, context) =>
      dependencies.echo.stopActivationTest(context.webContentsId),
    'shortcut-capture:start': async (_request, context) => ({
      leaseId: await dependencies.echo.startShortcutCapture(
        context.webContentsId,
        context.onDestroyed,
      ),
    }),
    'shortcut-capture:stop': async ({ leaseId }, context) => {
      await dependencies.echo.stopShortcutCapture(context.webContentsId, leaseId);
      return { accepted: true };
    },
    'window:minimize': (_request, context) => {
      dependencies.windows.getByWebContentsId(context.webContentsId)?.minimize();
      return { accepted: true };
    },
    'window:toggle-maximize': (_request, context) => {
      const window = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (window?.isMaximized()) window.unmaximize();
      else window?.maximize();
      return { maximized: window?.isMaximized() ?? false };
    },
    'window:close': async (_request, context) => {
      await dependencies.windows.closeMainByWebContentsId(context.webContentsId);
      return { accepted: true };
    },
    'widget:ready': (_request, context) => {
      dependencies.windows.markRendererReady('widget', context.webContentsId);
      dependencies.packagedMediaReady?.('widget');
      return dependencies.echo.snapshot;
    },
    'widget:stop': () => {
      dependencies.echo.stop();
      return { accepted: true };
    },
    'widget:cancel': () => {
      dependencies.echo.cancel();
      return { accepted: true };
    },
    'widget:set-interactive': ({ interactive }, context) => {
      dependencies.windows.setWidgetInteractive(context.webContentsId, interactive);
      return { accepted: true };
    },
    'capture:ready': (_request, context) => {
      const window = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (window !== null) dependencies.recording.attachCapture(window.webContents);
      dependencies.windows.markRendererReady('capture', context.webContentsId);
      dependencies.packagedMediaReady?.('capture');
      return { accepted: true };
    },
    'recording:get-devices': () => dependencies.recording.getDevices(),
    'recording:start-test': (_request, context) =>
      runUntilInvocationDestroyed(context.onDestroyed, (signal) =>
        dependencies.recording.startTest(
          dependencies.windows.getByWebContentsId(context.webContentsId)?.webContents ?? null,
          signal,
        ),
      ),
    'recording:stop-test': (_request, context) =>
      runUntilInvocationDestroyed(context.onDestroyed, (signal) =>
        dependencies.recording.stopTest(context.webContentsId, signal),
      ),
    'recording:open-microphone-settings': async () => {
      await dependencies.recording.openMicrophoneSettings();
      return { accepted: true };
    },
  };
}

async function runUntilInvocationDestroyed<Result>(
  onDestroyed: (listener: () => void) => () => void,
  operation: (signal: AbortSignal) => Promise<Result>,
): Promise<Result> {
  const controller = new AbortController();
  const removeDestroyedListener = onDestroyed(() => controller.abort());
  try {
    return await operation(controller.signal);
  } finally {
    removeDestroyedListener();
  }
}
