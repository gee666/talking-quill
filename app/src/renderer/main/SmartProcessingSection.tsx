import { useEffect, useRef, useState } from 'react';
import type { ProviderCatalogEntry } from '../../shared/schemas/providers';
import type { Settings } from '../../shared/schemas/settings';
import { Button, Card, EmptyState } from '../design';
import { ProviderConfigurationForm } from './smart-processing/ProviderConfigurationForm';
import {
  ConnectionTestPanel,
  CredentialPanel,
  DestinationSummary,
  PiInstallationPanel,
} from './smart-processing/ProviderPanels';
import { OnScreenAwarenessPanel, VisionVerificationDialog } from './smart-processing/VisionPanels';
import { ProviderPicker } from './smart-processing/ProviderPicker';
import { useOnScreenAwareness } from './smart-processing/useOnScreenAwareness';
import { usePiInstallation } from './smart-processing/usePiInstallation';
import { useProviderConfiguration } from './smart-processing/useProviderConfiguration';
import { useProviderOperations } from './smart-processing/useProviderOperations';
import { useProviderUiCoordinator } from './smart-processing/useProviderUiCoordinator';
import type { RequestState } from './smart-processing/provider-utils';
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
      configuration.selectedId === 'pi'
        ? JSON.stringify(configuration.savedDraft.piExtensionSources ?? [])
        : '',
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
    configuration.savedDraft.piExtensionSources,
    configuration.selectedId,
    configurationDirty,
    coordinator,
    operations,
  ]);

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

          <ProviderConfigurationForm
            selected={selected}
            configuration={configuration}
            operations={operations}
            controlsDisabled={providerMutationPending}
            configurationDirty={configurationDirty}
            externalMutationPending={pi.pathState === 'loading' || osa.mutationPending}
            clearMessage={pi.clearMessage}
          />

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
            configurationDirty={configurationDirty}
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
