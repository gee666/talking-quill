import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';

type ApplicationHandlerDependencies = Pick<
  HandlerDependencies,
  | 'acknowledgeDataReset'
  | 'appVersion'
  | 'applicationUpdates'
  | 'diagnosticSummary'
  | 'notices'
  | 'platform'
  | 'recording'
  | 'requestDataReset'
  | 'smart'
  | 'sourceRevision'
  | 'state'
  | 'systemInfo'
  | 'updateOperations'
  | 'updates'
  | 'welcome'
  | 'windows'
>;
type ApplicationHandlers = Pick<
  InvokeHandlerMap,
  | 'bootstrap:get'
  | 'welcome:set-step'
  | 'welcome:complete'
  | 'info:status'
  | 'info:check-update'
  | 'info:cancel-update'
  | 'info:update-state'
  | 'info:apply-update'
  | 'info:open-permission'
  | 'info:open-location'
  | 'info:open-release'
  | 'info:notices'
  | 'info:export-diagnostics'
  | 'data:reset-all'
  | 'data:reset-renderer-ack'
>;

export function createApplicationHandlers(
  dependencies: ApplicationHandlerDependencies,
): ApplicationHandlers {
  return {
    'bootstrap:get': () => ({
      appVersion: dependencies.appVersion,
      sourceRevision: dependencies.sourceRevision,
      platform: dependencies.platform,
      state: dependencies.state.getState(),
      settings: dependencies.state.getSettings(),
    }),
    'welcome:set-step': ({ step }) => dependencies.welcome.setStep(step),
    'welcome:complete': () => dependencies.welcome.complete(),
    'info:status': async () => ({
      microphone: (await dependencies.recording.getDevices()).permission,
      screenRecording: dependencies.smart.status().screenPermission,
      helper: dependencies.state.getState().helper,
    }),
    'info:check-update': async ({ operationId }, context) => {
      const result = await dependencies.updateOperations.run(context, operationId, (signal) =>
        dependencies.updates.check(dependencies.appVersion, signal),
      );
      await dependencies.applicationUpdates.acceptCheckResult(result);
      return result;
    },
    'info:cancel-update': ({ operationId }, context) => ({
      cancelled: dependencies.updateOperations.cancel(context.webContentsId, operationId),
    }),
    'info:update-state': () => dependencies.applicationUpdates.getState(),
    'info:apply-update': () => dependencies.applicationUpdates.apply(),
    'info:open-permission': async ({ permission }) => {
      await dependencies.systemInfo.openPermission(permission);
      return { accepted: true };
    },
    'info:open-location': async ({ location }) => {
      await dependencies.systemInfo.openLocation(location);
      return { accepted: true };
    },
    'info:open-release': async ({ url }) => {
      await dependencies.systemInfo.openRelease(url);
      return { accepted: true };
    },
    'info:notices': async () => ({ text: await dependencies.notices.read() }),
    'info:export-diagnostics': async (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Diagnostic export dialog owner is unavailable');
      return {
        status: await dependencies.systemInfo.exportDiagnostics(
          owner,
          dependencies.diagnosticSummary(),
        ),
      };
    },
    'data:reset-all': async () => ({
      accepted: true,
      acknowledgementToken: await dependencies.requestDataReset(),
    }),
    'data:reset-renderer-ack': ({ acknowledgementToken }) => {
      dependencies.acknowledgeDataReset(acknowledgementToken);
      return { accepted: true };
    },
  };
}
