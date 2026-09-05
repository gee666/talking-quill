import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';

type ProviderHandlerDependencies = Pick<
  HandlerDependencies,
  | 'piInstallation'
  | 'providerConfigs'
  | 'providerMutations'
  | 'providerOperations'
  | 'providers'
  | 'smart'
  | 'windows'
>;
type ProviderHandlers = Pick<
  InvokeHandlerMap,
  | 'provider:catalog'
  | 'provider:pi-installation-status'
  | 'provider:pi-installation-save'
  | 'provider:pi-installation-browse'
  | 'provider:config-save'
  | 'provider:secret-set'
  | 'provider:secret-status'
  | 'provider:secret-delete'
  | 'provider:list-models'
  | 'provider:test-connection'
  | 'provider:destination'
  | 'provider:cancel'
  | 'provider:osa-status'
  | 'provider:osa-set'
  | 'provider:vision-test'
  | 'provider:vision-confirm'
>;

export function createProviderHandlers(
  dependencies: ProviderHandlerDependencies,
): ProviderHandlers {
  return {
    'provider:catalog': () => ({ providers: [...dependencies.providers.catalog()] }),
    'provider:pi-installation-status': () => dependencies.piInstallation.status(),
    'provider:pi-installation-save': ({ path }) => dependencies.piInstallation.save(path),
    'provider:pi-installation-browse': async (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Pi installation dialog owner is unavailable');
      return { path: await dependencies.piInstallation.browse(owner) };
    },
    'provider:config-save': ({ config }) =>
      dependencies.providerMutations.saveConfigWithCredentialState(config),
    'provider:secret-set': ({ providerId, expectedBindingToken, secret }) =>
      dependencies.providerMutations.setSecret(providerId, expectedBindingToken, secret),
    'provider:secret-status': ({ providerId }) =>
      dependencies.providerMutations.secretStatus(providerId),
    'provider:secret-delete': ({ providerId, expectedBindingToken }) =>
      dependencies.providerMutations.deleteSecret(providerId, expectedBindingToken),
    'provider:list-models': ({ providerId, operationId, refresh }, context) =>
      dependencies.providerOperations.run(context, operationId, async (signal) => ({
        providerId,
        models: [
          ...(await dependencies.providers.listModels(
            dependencies.providerConfigs.get(providerId),
            signal,
            { refresh },
          )),
        ],
      })),
    'provider:test-connection': ({ providerId, operationId }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.providers.testConnection(dependencies.providerConfigs.get(providerId), signal),
      ),
    'provider:destination': ({ providerId, operationId }, context) =>
      dependencies.providerOperations.run(context, operationId, async (signal) => ({
        destination: await dependencies.providers.classifyDestination(
          dependencies.providerConfigs.get(providerId),
          signal,
        ),
      })),
    'provider:cancel': ({ operationId }, context) => ({
      cancelled: dependencies.providerOperations.cancel(context.webContentsId, operationId),
    }),
    'provider:osa-status': () => dependencies.smart.status(),
    'provider:osa-set': ({ enabled }) => dependencies.smart.setOnScreenAwareness(enabled),
    'provider:vision-test': ({ operationId, nonce }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.smart.verifyManualVision(nonce, signal),
      ),
    'provider:vision-confirm': ({ operationId, verificationId }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.smart.confirmManualVision(verificationId, signal),
      ),
  };
}
