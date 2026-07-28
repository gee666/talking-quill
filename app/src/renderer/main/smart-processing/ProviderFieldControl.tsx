import { useMemo, useState } from 'react';
import type {
  ModelInfo,
  ProviderCatalogEntry,
  ProviderField,
} from '../../../shared/schemas/providers';
import type { ProviderSettingsDraft } from '../../../shared/schemas/settings';
import { Button, Input, Select, Status } from '../../design';
import { formatOperationElapsed, type RequestState } from './provider-utils';

const MAX_VISIBLE_MODELS = 200;

/**
 * Sentinel option value that switches the single model select into free-text entry. It contains a
 * NUL so it can never collide with a real model id (Ollama model names are user-chosen).
 */
export const CUSTOM_MODEL_OPTION = '\u0000custom-model';

export function ProviderFieldControl({
  field,
  value,
  models,
  modelState,
  modelElapsedMs,
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
  readonly modelElapsedMs: number;
  readonly modelDiscovery: ProviderCatalogEntry['modelDiscovery'];
  readonly error?: string | undefined;
  readonly controlsDisabled: boolean;
  readonly operationsDisabled: boolean;
  readonly onChange: (value: ProviderSettingsDraft[keyof ProviderSettingsDraft]) => void;
  readonly onDiscover: () => void;
  readonly onCancel: () => void;
}) {
  const [customChosen, setCustomChosen] = useState(false);
  const [modelSearch, setModelSearch] = useState('');
  const modelValue = typeof value === 'string' ? value : '';
  const outsideCatalog =
    field.kind === 'model' &&
    modelValue.length > 0 &&
    !models.some((model) => model.id === modelValue);
  const customMode = field.kind === 'model' && (customChosen || outsideCatalog);
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
    if (modelDiscovery === 'azure-deployment') {
      return (
        <div className="model-field stack">
          <Input
            label={field.label}
            required={field.required}
            value={modelValue}
            placeholder={field.placeholder}
            error={error}
            disabled={controlsDisabled}
            hint="Type the deployment name exactly as you created it in Azure."
            onChange={(event) => onChange(event.currentTarget.value)}
          />
          <Status tone="info">
            Azure keeps its deployment list behind a separate management login, so Talking Quill
            cannot list them for you.
          </Status>
        </div>
      );
    }

    // The surrounding section already explains that a provider-managed service picks its own model.
    if (modelDiscovery === 'provider-managed') return null;

    const discovering = modelState === 'loading';
    const selectValue = customMode ? CUSTOM_MODEL_OPTION : modelValue;
    const searchNeeded = !customMode && models.length > MAX_VISIBLE_MODELS;

    return (
      <div className="model-field stack">
        {discovering ? (
          <Status tone="info" live>
            Looking for available models… {formatOperationElapsed(modelElapsedMs)}
          </Status>
        ) : null}
        {searchNeeded ? (
          <Input
            type="search"
            label="Search models"
            value={modelSearch}
            disabled={controlsDisabled}
            hint={`There are a lot of models here. Type to narrow the list down to ${String(MAX_VISIBLE_MODELS)} at a time.`}
            onChange={(event) => setModelSearch(event.currentTarget.value)}
          />
        ) : null}
        <Select
          label={field.label}
          required={field.required}
          value={selectValue}
          hint={field.description}
          error={customMode ? undefined : error}
          disabled={controlsDisabled}
          onChange={(event) => {
            const next = event.currentTarget.value;
            if (next === CUSTOM_MODEL_OPTION) {
              setCustomChosen(true);
              return;
            }
            setCustomChosen(false);
            setModelSearch('');
            onChange(next);
          }}
        >
          {selectValue === '' ? <option value="">Choose a model…</option> : null}
          {visibleModels.map((model) => (
            <option key={model.id} value={model.id}>
              {model.name}
            </option>
          ))}
          <option value={CUSTOM_MODEL_OPTION}>Type a model name myself…</option>
        </Select>
        {customMode ? (
          <Input
            label={`${field.label} name`}
            required={field.required}
            value={modelValue}
            placeholder={field.placeholder}
            error={error}
            disabled={controlsDisabled}
            hint="Type the exact model name, for example llama3.1:8b."
            onChange={(event) => {
              // Typing here keeps free-text mode until a real option is picked, so an arriving
              // catalogue cannot swap this control for the picker mid-keystroke.
              setCustomChosen(true);
              onChange(event.currentTarget.value);
            }}
          />
        ) : null}
        <div className="provider-actions">
          <Button
            variant="quiet"
            busy={discovering}
            disabled={controlsDisabled || operationsDisabled}
            onClick={onDiscover}
          >
            Refresh list
          </Button>
          {discovering ? (
            <Button variant="quiet" onClick={onCancel}>
              Stop searching
            </Button>
          ) : null}
          {modelState === 'empty' ? (
            <Status tone="warning">No models found — you can type a name instead.</Status>
          ) : null}
          {modelState === 'success' ? (
            <Status tone="success">
              {String(models.length)} {models.length === 1 ? 'model' : 'models'} found
            </Status>
          ) : null}
        </div>
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
