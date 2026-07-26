import { useEffect, useRef, useState, type SyntheticEvent } from 'react';
import type { ProviderCatalogEntry } from '../../shared/schemas/providers';
import type { ProviderSettingsDraft, Settings } from '../../shared/schemas/settings';
import { Button, Card, EmptyState, Status } from '../design';
import { ProviderFieldControl } from './smart-processing/ProviderFieldControl';
import {
  ConnectionTestPanel,
  CredentialPanel,
  DestinationSummary,
  OnScreenAwarenessPanel,
  PiInstallationPanel,
  VisionVerificationDialog,
} from './smart-processing/ProviderPanels';
import { ProviderPicker } from './smart-processing/ProviderPicker';
import { useOnScreenAwareness } from './smart-processing/useOnScreenAwareness';
import { usePiInstallation } from './smart-processing/usePiInstallation';
import { useProviderConfiguration } from './smart-processing/useProviderConfiguration';
import { useProviderOperations } from './smart-processing/useProviderOperations';
import { useProviderUiCoordinator } from './smart-processing/useProviderUiCoordinator';
import { ENDPOINT_REPAIR_MESSAGE, type RequestState } from './smart-processing/provider-utils';

interface SmartProcessingSectionProps {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}

export function SmartProcessingSection({ settings, onSettingsSaved }: SmartProcessingSectionProps) {
  const [catalog, setCatalog] = useState<readonly ProviderCatalogEntry[]>([]);
  const [catalogState, setCatalogState] = useState<RequestState>('loading');
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const coordinator = useProviderUiCoordinator(settings.smartProcessing.selectedProviderId);
  const operations = useProviderOperations(coordinator);
  const configuration = useProviderConfiguration({
    settings,
    onSettingsSaved,
    coordinator,
    operations,
  });
  const pi = usePiInstallation({
    active: configuration.selectedId === 'pi',
    configuredPath: settings.smartProcessing.piInstallationPath,
    draft: configuration.draft,
    coordinator,
    operations,
  });
  const osa = useOnScreenAwareness({
    settings,
    selectedId: configuration.selectedId,
    savedDraft: configuration.savedDraft,
    providerSelectionPersisted: configuration.providerSelectionPersisted,
    savePending: configuration.saveState === 'loading',
    credentialMutationPending: configuration.credentialMutationPending,
    dirty:
      configuration.dirty ||
      configuration.endpointRepairRequired ||
      !configuration.providerSelectionPersisted,
    coordinator,
    onSettingsSaved,
    onError: operations.setConnectionError,
  });
  const initialDraftRef = useRef(configuration.draft);
  const discoverPiImmediately = operations.discoverPiImmediately;

  useEffect(() => {
    let active = true;
    void window.talkingQuill.providers
      .catalog()
      .then((providers) => {
        if (!active) return;
        setCatalog(providers);
        setCatalogState(providers.length === 0 ? 'empty' : 'success');
        if (coordinator.current().providerId === 'pi') {
          void discoverPiImmediately(initialDraftRef.current, coordinator.current());
        }
      })
      .catch(() => {
        if (!active) return;
        setCatalogError('The provider catalog could not be loaded.');
        setCatalogState('error');
      });
    return () => {
      active = false;
    };
  }, [coordinator, discoverPiImmediately]);

  const selected = catalog.find((provider) => provider.id === configuration.selectedId) ?? null;
  const configurableEndpoint = selected?.fields.some((field) => field.key === 'baseUrl') ?? false;
  const providerManagedModel = selected?.modelDiscovery === 'provider-managed';
  const missingRequiredModel =
    !providerManagedModel &&
    (configuration.draft.modelId === undefined || configuration.draft.modelId === null);
  const displayedDestination =
    configurableEndpoint && !operations.destinationVerified
      ? null
      : (operations.destination ?? selected?.destinationHint ?? null);
  const modelMessage = pi.message ?? operations.modelMessage;
  const providerMutationPending =
    configuration.saveState === 'loading' ||
    pi.pathState === 'loading' ||
    configuration.credentialMutationPending ||
    osa.mutationPending;
  const connectionBlocked =
    providerMutationPending ||
    configuration.providerSelectionPending ||
    !configuration.providerSelectionPersisted ||
    configuration.endpointRepairRequired ||
    configuration.dirty ||
    missingRequiredModel;

  const saveConfiguration = (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => {
    event.preventDefault();
    pi.clearMessage();
    void configuration.saveConfiguration(pi.pathState === 'loading' || osa.mutationPending);
  };

  return (
    <Card
      title="Smart processing"
      description="Choose where transcript cleanup runs. Provider secrets stay in encrypted main-process storage."
    >
      {catalogState === 'loading' ? (
        <p role="status" aria-live="polite">
          Loading providers…
        </p>
      ) : null}
      {catalogState === 'error' ? (
        <EmptyState
          title="Provider catalog unavailable"
          description={catalogError ?? 'Retry after restarting Talking Quill.'}
          action={
            <Button variant="secondary" onClick={() => window.location.reload()}>
              Retry
            </Button>
          }
        />
      ) : null}
      {catalogState === 'empty' ? (
        <EmptyState title="No providers" description="No provider definitions are available." />
      ) : null}
      {catalogState === 'success' && selected !== null ? (
        <>
          <ProviderPicker
            providers={catalog}
            selected={selected}
            disabled={providerMutationPending}
            onSelect={(provider) => {
              pi.clearMessage();
              void configuration.selectProvider(
                provider,
                pi.pathState === 'loading' || osa.mutationPending,
              );
            }}
          />

          <DestinationSummary
            destination={displayedDestination}
            verified={operations.destinationVerified}
          />

          {configuration.selectedId === 'pi' ? (
            <PiInstallationPanel
              path={pi.path}
              pathState={pi.pathState}
              disabled={
                configuration.saveState === 'loading' ||
                configuration.credentialMutationPending ||
                osa.mutationPending
              }
              installation={pi.installation}
              modelState={operations.modelState}
              modelElapsedMs={operations.modelElapsedMs}
              onPathChange={pi.setPath}
              onAction={(action) =>
                void pi.run(
                  action,
                  configuration.saveState === 'loading' ||
                    configuration.credentialMutationPending ||
                    osa.mutationPending,
                )
              }
            />
          ) : null}

          <form className="provider-form" onSubmit={saveConfiguration}>
            {selected.fields
              .filter((field) => !field.secret)
              .map((field) => (
                <ProviderFieldControl
                  key={field.key}
                  field={field}
                  value={configuration.draft[field.key as keyof ProviderSettingsDraft]}
                  models={operations.models}
                  modelState={operations.modelState}
                  modelDiscovery={selected.modelDiscovery}
                  error={
                    field.key === 'baseUrl' && configuration.endpointRepairRequired
                      ? ENDPOINT_REPAIR_MESSAGE
                      : configuration.fieldErrors[field.key]
                  }
                  controlsDisabled={
                    configuration.saveState === 'loading' ||
                    pi.pathState === 'loading' ||
                    configuration.credentialMutationPending ||
                    osa.mutationPending
                  }
                  operationsDisabled={
                    !configuration.providerSelectionPersisted ||
                    configuration.endpointRepairRequired ||
                    (configuration.dirty && configuration.selectedId !== 'pi')
                  }
                  onChange={(value) => {
                    pi.clearMessage();
                    configuration.updateDraft(field.key as keyof ProviderSettingsDraft, value);
                  }}
                  onDiscover={() => {
                    pi.clearMessage();
                    void operations.discoverModels({
                      providerId: configuration.selectedId,
                      draft: configuration.draft,
                      configurationDirty:
                        configuration.dirty ||
                        configuration.endpointRepairRequired ||
                        !configuration.providerSelectionPersisted,
                    });
                  }}
                  onCancel={operations.cancelModelDiscovery}
                />
              ))}
            {selected.modelDiscovery === 'provider-managed' ? (
              <Status tone="info">
                This provider uses its currently loaded model; Talking Quill does not select one.
              </Status>
            ) : null}
            <div className="provider-actions">
              <Button
                type="submit"
                busy={configuration.saveState === 'loading'}
                disabled={
                  !configuration.dirty ||
                  configuration.endpointRepairRequired ||
                  pi.pathState === 'loading' ||
                  osa.mutationPending
                }
              >
                Save configuration
              </Button>
              {configuration.dirty ? (
                <Status tone="warning">Save changes before discovery or testing</Status>
              ) : null}
              {configuration.saveState === 'success' ? (
                <Status tone="success">Draft saved</Status>
              ) : null}
              {configuration.saveState === 'error' ? (
                <Status tone="error">Configuration update failed; status refreshed</Status>
              ) : null}
            </div>
          </form>

          {selected.fields.some((field) => field.secret) ? (
            <CredentialPanel
              key={configuration.selectedId}
              providerId={configuration.selectedId}
              configured={configuration.credentialConfigured}
              bindingDirty={configuration.credentialBindingDirty}
              state={configuration.credentialState}
              dirty={configuration.dirty}
              disabled={
                configuration.providerSelectionPending ||
                configuration.saveState === 'loading' ||
                pi.pathState === 'loading' ||
                osa.mutationPending ||
                configuration.endpointRepairRequired ||
                !configuration.providerSelectionPersisted
              }
              accessKeyRef={configuration.awsAccessKeyRef}
              secretRef={configuration.secretRef}
              sessionTokenRef={configuration.awsSessionTokenRef}
              onSave={() =>
                void configuration.saveSecret(pi.pathState === 'loading' || osa.mutationPending)
              }
              onDelete={() =>
                void configuration.deleteSecret(pi.pathState === 'loading' || osa.mutationPending)
              }
            />
          ) : null}

          <ConnectionTestPanel
            state={operations.connectionState}
            message={operations.connectionMessage}
            elapsedMs={operations.connectionElapsedMs}
            disabled={connectionBlocked}
            configurationDirty={
              configuration.dirty ||
              configuration.endpointRepairRequired ||
              !configuration.providerSelectionPersisted
            }
            missingModel={missingRequiredModel}
            providerManagedModel={providerManagedModel}
            onTest={() =>
              void operations.testConnection({
                providerId: configuration.selectedId,
                blocked: connectionBlocked,
              })
            }
            onCancel={operations.cancelConnectionTest}
          />

          <OnScreenAwarenessPanel
            enabled={settings.smartProcessing.onScreenAwarenessEnabled}
            controlsEnabled={osa.controlsEnabled}
            capability={osa.capability}
            manualVisionAllowed={osa.manualAllowed}
            screenPermission={osa.screenPermission}
            onUpdate={(enabled) => void osa.update(enabled)}
            onBeginVisionTest={osa.beginTest}
          />

          <VisionVerificationDialog
            open={osa.dialogOpen}
            nonce={osa.nonce}
            state={osa.testState}
            commitPending={osa.commitPending}
            controlsEnabled={osa.controlsEnabled}
            onClose={osa.cancelTest}
            onVerify={() => void osa.verify()}
          />

          {modelMessage === null ? null : (
            <p
              className={`operation-message operation-message--${operations.modelState}`}
              role="status"
            >
              {modelMessage}
            </p>
          )}
        </>
      ) : null}
    </Card>
  );
}
