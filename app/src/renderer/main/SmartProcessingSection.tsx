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
import { autoDiscoveryKey, claimAutoDiscovery } from './smart-processing/auto-discovery-memory';

interface SmartProcessingSectionProps {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
  /** Pass `null` when the surrounding screen already shows this heading. */
  readonly heading?: string | null;
  /**
   * Whether the user deliberately opened this section. Call sites that surface the section without
   * an explicit request (for example a settings search that happens to match it) must pass `false`,
   * so no AI service is contacted for a configuration the user never asked to see.
   */
  readonly autoDiscover?: boolean;
}

export function SmartProcessingSection({
  settings,
  onSettingsSaved,
  heading = 'Smart processing',
  autoDiscover = true,
}: SmartProcessingSectionProps) {
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

  const configurationDirty =
    configuration.dirty ||
    configuration.endpointRepairRequired ||
    !configuration.providerSelectionPersisted;
  const credentialRequired =
    selected?.fields.some((field) => field.secret && field.required) ?? false;

  // Auto-discovery replaces the old "Discover models" click. It runs at most once per persisted
  // configuration, only from a fresh idle state, so an error or a cancellation is never retried in
  // a loop, and only for a provider that can actually be reached without a user click.
  const autoDiscoveryAllowed =
    autoDiscover &&
    selected !== null &&
    selected.modelDiscovery === 'remote' &&
    catalogState === 'success' &&
    (!credentialRequired || configuration.credentialConfigured) &&
    configuration.providerSelectionPersisted &&
    !configuration.providerSelectionPending &&
    !configuration.endpointRepairRequired &&
    !configuration.dirty &&
    !providerMutationPending &&
    operations.modelState === 'idle';

  useEffect(() => {
    if (!autoDiscoveryAllowed) return;
    const attempt = autoDiscoveryKey(
      configuration.selectedId,
      configuration.persistedCredentialBinding,
      configuration.credentialEpoch,
    );
    if (!claimAutoDiscovery(attempt)) return;
    void operations.discoverModelsQuietly({
      providerId: configuration.selectedId,
      draft: configuration.draft,
      configurationDirty,
      expectedLease: coordinator.current(),
    });
  }, [
    autoDiscoveryAllowed,
    configuration.credentialEpoch,
    configuration.draft,
    configuration.persistedCredentialBinding,
    configuration.selectedId,
    configurationDirty,
    coordinator,
    operations,
  ]);

  const saveConfiguration = (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => {
    event.preventDefault();
    pi.clearMessage();
    void configuration.saveConfiguration(pi.pathState === 'loading' || osa.mutationPending);
  };

  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="Smart dictation hands what you said to an AI service that tidies it up — punctuation, capitals, stray filler words. Raw dictation needs none of this and never leaves your computer. A service running on your own machine, like Ollama, keeps everything here; a cloud service sends your text to a company that may charge you for it."
    >
      {catalogState === 'loading' ? (
        <p role="status" aria-live="polite">
          Loading AI services…
        </p>
      ) : null}
      {catalogState === 'error' ? (
        <EmptyState
          title="We could not load the list of AI services"
          description={catalogError ?? 'Restart Talking Quill and try again.'}
          action={
            <Button variant="secondary" onClick={() => window.location.reload()}>
              Retry
            </Button>
          }
        />
      ) : null}
      {catalogState === 'empty' ? (
        <EmptyState
          title="No AI services available"
          description="Restart Talking Quill and try again."
        />
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
            providerName={selected.displayName}
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

          <form className="stack" onSubmit={saveConfiguration}>
            {selected.fields
              .filter((field) => !field.secret)
              .map((field) => (
                <ProviderFieldControl
                  key={`${configuration.selectedId}-${field.key}`}
                  field={field}
                  value={configuration.draft[field.key as keyof ProviderSettingsDraft]}
                  models={operations.models}
                  modelState={operations.modelState}
                  modelElapsedMs={operations.modelElapsedMs}
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
                This service uses the model it already has loaded, so there is no model to choose.
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
                <Status tone="warning">Save your changes before testing</Status>
              ) : null}
              {configuration.saveState === 'success' ? <Status tone="success">Saved</Status> : null}
              {configuration.saveState === 'error' ? (
                <Status tone="error">That did not save. Check the settings and try again.</Status>
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
