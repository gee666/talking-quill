import type { SyntheticEvent } from 'react';
import type { ProviderCatalogEntry } from '../../../shared/schemas/providers';
import { Button, Status } from '../../design';
import { ProviderFieldControl } from './ProviderFieldControl';
import { ENDPOINT_REPAIR_MESSAGE } from './provider-utils';
import type { ProviderConfigurationController } from './useProviderConfiguration';
import type { ProviderOperationsController } from './useProviderOperations';

export function ProviderConfigurationForm({
  selected,
  configuration,
  operations,
  controlsDisabled,
  configurationDirty,
  externalMutationPending,
  clearMessage,
}: {
  readonly selected: ProviderCatalogEntry;
  readonly configuration: ProviderConfigurationController;
  readonly operations: ProviderOperationsController;
  readonly controlsDisabled: boolean;
  readonly configurationDirty: boolean;
  readonly externalMutationPending: boolean;
  readonly clearMessage: () => void;
}) {
  const saveConfiguration = (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => {
    event.preventDefault();
    clearMessage();
    void configuration.saveConfiguration(externalMutationPending);
  };

  return (
    <form className="stack" onSubmit={saveConfiguration}>
      {selected.fields
        .filter((field) => !field.secret)
        .map((field) => (
          <ProviderFieldControl
            key={`${configuration.selectedId}-${field.key}`}
            field={field}
            value={field.key === 'credential' ? undefined : configuration.draft[field.key]}
            models={operations.models}
            modelState={operations.modelState}
            modelElapsedMs={operations.modelElapsedMs}
            modelDiscovery={selected.modelDiscovery}
            error={
              field.key === 'baseUrl' && configuration.endpointRepairRequired
                ? ENDPOINT_REPAIR_MESSAGE
                : configuration.fieldErrors[field.key]
            }
            controlsDisabled={controlsDisabled}
            operationsDisabled={configurationDirty}
            onChange={(value) => {
              clearMessage();
              if (field.key !== 'credential') configuration.updateDraft(field.key, value);
            }}
            onDiscover={() => {
              clearMessage();
              void operations.discoverModels({
                providerId: configuration.selectedId,
                draft: configuration.draft,
                configurationDirty,
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
            !configuration.dirty || configuration.endpointRepairRequired || externalMutationPending
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
  );
}
