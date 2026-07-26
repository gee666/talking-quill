import { useMemo, useState } from 'react';
import type {
  ModelInfo,
  ProviderCatalogEntry,
  ProviderField,
} from '../../../shared/schemas/providers';
import type { ProviderSettingsDraft } from '../../../shared/schemas/settings';
import { Button, Input, Select, Status } from '../../design';
import type { RequestState } from './provider-utils';

const MAX_VISIBLE_MODELS = 200;

export function ProviderFieldControl({
  field,
  value,
  models,
  modelState,
  modelDiscovery,
  error,
  controlsDisabled,
  operationsDisabled,
  onChange,
  onDiscover,
  onCancel,
}: {
  readonly field: ProviderField;
  readonly value: ProviderSettingsDraft[keyof ProviderSettingsDraft];
  readonly models: readonly ModelInfo[];
  readonly modelState: RequestState;
  readonly modelDiscovery: ProviderCatalogEntry['modelDiscovery'];
  readonly error?: string | undefined;
  readonly controlsDisabled: boolean;
  readonly operationsDisabled: boolean;
  readonly onChange: (value: ProviderSettingsDraft[keyof ProviderSettingsDraft]) => void;
  readonly onDiscover: () => void;
  readonly onCancel: () => void;
}) {
  const [manualModelEntry, setManualModelEntry] = useState(false);
  const [modelSearch, setModelSearch] = useState('');
  const modelValue = typeof value === 'string' ? value : '';
  const selectedOutsideCatalog =
    field.kind === 'model' &&
    modelValue.length > 0 &&
    !models.some((model) => model.id === modelValue);
  const showManualEntry =
    field.kind === 'model' && (models.length === 0 || manualModelEntry || selectedOutsideCatalog);
  const visibleModels = useMemo(() => {
    if (field.kind !== 'model') return [];
    const query = modelSearch.trim().toLocaleLowerCase();
    const matches =
      query.length === 0
        ? models
        : models.filter((model) => `${model.id} ${model.name}`.toLocaleLowerCase().includes(query));
    const bounded = matches.slice(0, MAX_VISIBLE_MODELS);
    const selected = models.find((model) => model.id === modelValue);
    return selected === undefined || bounded.some((model) => model.id === selected.id)
      ? bounded
      : [selected, ...bounded.slice(0, MAX_VISIBLE_MODELS - 1)];
  }, [field.kind, modelSearch, modelValue, models]);

  if (field.kind === 'model') {
    return (
      <div className="model-field">
        {showManualEntry ? (
          <Input
            label={field.label}
            required={field.required}
            value={modelValue}
            placeholder={field.placeholder}
            error={error}
            disabled={controlsDisabled}
            hint="Enter the exact provider/model ID. Discovery never clears a manual selection."
            onChange={(event) => onChange(event.currentTarget.value)}
          />
        ) : (
          <>
            {models.length > MAX_VISIBLE_MODELS ? (
              <Input
                type="search"
                label="Search discovered models"
                value={modelSearch}
                disabled={controlsDisabled}
                hint={`Showing at most ${String(MAX_VISIBLE_MODELS)} matching models.`}
                onChange={(event) => setModelSearch(event.currentTarget.value)}
              />
            ) : null}
            <Select
              label={field.label}
              required={field.required}
              value={modelValue}
              error={error}
              disabled={controlsDisabled}
              onChange={(event) => onChange(event.currentTarget.value)}
            >
              <option value="">Select a discovered model</option>
              {visibleModels.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.name}
                </option>
              ))}
            </Select>
          </>
        )}
        {models.length > 0 ? (
          <Button
            variant="quiet"
            disabled={controlsDisabled}
            onClick={() => {
              setModelSearch('');
              if (showManualEntry) {
                if (selectedOutsideCatalog) onChange('');
                setManualModelEntry(false);
              } else {
                setManualModelEntry(true);
              }
            }}
          >
            {showManualEntry ? 'Choose a discovered model' : 'Enter model ID manually'}
          </Button>
        ) : null}
        {modelDiscovery === 'remote' ? (
          <div className="provider-actions">
            <Button
              variant="secondary"
              busy={modelState === 'loading'}
              disabled={controlsDisabled || operationsDisabled}
              onClick={onDiscover}
            >
              {modelState === 'error' || modelState === 'cancelled'
                ? 'Retry discovery'
                : 'Discover models'}
            </Button>
            {modelState === 'loading' ? (
              <Button variant="quiet" onClick={onCancel}>
                Cancel discovery
              </Button>
            ) : null}
            {modelState === 'empty' ? <Status tone="warning">No models found</Status> : null}
            {modelState === 'success' ? (
              <Status tone="success">
                {String(models.length)} {models.length === 1 ? 'model' : 'models'} found
              </Status>
            ) : null}
          </div>
        ) : modelDiscovery === 'azure-deployment' ? (
          <Status tone="info">
            Deployment discovery requires Azure management-plane credentials.
          </Status>
        ) : (
          <Status tone="info">
            This provider uses its currently loaded model; Talking Quill does not select one.
          </Status>
        )}
      </div>
    );
  }

  if (field.kind === 'select' && field.options !== undefined) {
    return (
      <Select
        label={field.label}
        required={field.required}
        value={value === undefined || value === null ? '' : String(value)}
        hint={field.description}
        error={error}
        disabled={controlsDisabled}
        onChange={(event) => {
          const option = field.options?.find(
            (candidate) => String(candidate.value) === event.currentTarget.value,
          );
          onChange(option?.value);
        }}
      >
        {field.options.map((option) => (
          <option key={String(option.value)} value={String(option.value)}>
            {option.label}
          </option>
        ))}
      </Select>
    );
  }

  const numberField = field.kind === 'number';
  return (
    <Input
      label={field.label}
      type={numberField ? 'number' : field.kind === 'url' ? 'url' : 'text'}
      required={field.required}
      min={field.min}
      max={field.max}
      value={value === undefined || value === null ? '' : String(value)}
      placeholder={field.placeholder}
      hint={field.description}
      error={error}
      disabled={controlsDisabled}
      onChange={(event) => {
        const raw = event.currentTarget.value;
        onChange(numberField ? (raw.length === 0 ? undefined : Number(raw)) : raw);
      }}
    />
  );
}
