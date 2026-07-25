import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type SyntheticEvent,
} from 'react';
import { isRunnableProviderId } from '../../shared/bridge/api';
import { rendererWatchdog } from './provider-watchdog';
import { serializeAwsCredentials } from '../../shared/schemas/credentials';
import {
  RunnableProviderConfigSchema,
  type Destination,
  type ModelInfo,
  type ProviderCatalogEntry,
  type ProviderField,
  type RunnableProviderConfig,
  type RunnableProviderId,
  type VisionCapability,
} from '../../shared/schemas/providers';
import {
  ProviderSettingsDraftSchema,
  type ProviderSettingsDraft,
  type Settings,
} from '../../shared/schemas/settings';
import type { PiInstallationStatus } from '../../shared/schemas/pi-installation';
import { Button, Card, Dialog, EmptyState, Input, Select, Status, Toggle } from '../design';
import { PROVIDER_LOGOS } from './provider-logos';
import { reconcileDiscoveredModels } from './pi-model-selection';

interface SmartProcessingSectionProps {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}

type RequestState = 'idle' | 'loading' | 'success' | 'empty' | 'error' | 'cancelled';
type ConnectionState = 'idle' | 'loading' | 'success' | 'error' | 'cancelled';

const ACTIONABLE_ERRORS: Readonly<Record<string, string>> = Object.freeze({
  INVALID_CONFIG: 'Check the endpoint and required configuration fields.',
  STALE_CONFIG: 'The provider configuration changed. Refresh it and enter the credential again.',
  MISSING_CREDENTIAL: 'Store an API key before connecting to this provider.',
  SECURITY_BLOCKED: 'This endpoint was blocked. Use HTTPS for credentialed LAN or cloud services.',
  UNAVAILABLE: 'The provider is unavailable. Check that it is running and reachable.',
  PI_NOT_FOUND: 'A compatible Pi executable was not found. Install Pi or choose its path.',
  PI_CONFIG_INVALID: 'The configured Pi executable or directory is invalid.',
  PI_INCOMPATIBLE:
    'This Pi command does not expose the required print, model, thinking, and list options.',
  PI_LAUNCH_FAILED: 'Pi could not start or did not complete its bounded capability check.',
  AUTHENTICATION_FAILED: 'The provider rejected the API key. Replace it and try again.',
  RATE_LIMITED: 'The provider rate limit was reached. Wait, then retry.',
  MODEL_NOT_FOUND: 'The selected model is not installed or no longer available.',
  NO_MODELS: 'No compatible models were found. Install one or enter its model ID manually.',
  TIMEOUT: 'The provider did not respond before the timeout. Check the endpoint and retry.',
  CANCELLED: 'The provider request was cancelled.',
  REQUEST_TOO_LARGE: 'The transcript is too large for one provider request.',
  RESPONSE_TOO_LARGE: 'The provider returned more data than Talking Quill accepts.',
  INVALID_RESPONSE: 'The provider response was not valid. Check provider compatibility.',
  REMOTE_FAILURE: 'The provider reported an error. Check its status and configuration.',
});

let operationSequence = 0;

export function SmartProcessingSection({ settings, onSettingsSaved }: SmartProcessingSectionProps) {
  const [catalog, setCatalog] = useState<readonly ProviderCatalogEntry[]>([]);
  const [catalogState, setCatalogState] = useState<RequestState>('loading');
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [activeOption, setActiveOption] = useState(0);
  const [selectedId, setSelectedId] = useState(settings.smartProcessing.selectedProviderId);
  const [draft, setDraft] = useState<ProviderSettingsDraft>(
    () => settings.smartProcessing.providers[settings.smartProcessing.selectedProviderId] ?? {},
  );
  const [fieldErrors, setFieldErrors] = useState<Readonly<Record<string, string>>>({});
  const [saveState, setSaveState] = useState<RequestState>('idle');
  const [credentialConfigured, setCredentialConfigured] = useState(false);
  const [credentialStatusBinding, setCredentialStatusBinding] = useState<string | null>(null);
  const [credentialBindingToken, setCredentialBindingToken] = useState<string | null>(null);
  const [credentialState, setCredentialState] = useState<RequestState>('loading');
  const [models, setModels] = useState<readonly ModelInfo[]>([]);
  const [modelState, setModelState] = useState<RequestState>('idle');
  const [modelMessage, setModelMessage] = useState<string | null>(null);
  const [modelElapsedMs, setModelElapsedMs] = useState(0);
  const [piInstallation, setPiInstallation] = useState<PiInstallationStatus | null>(null);
  const [piPathInput, setPiPathInput] = useState(settings.smartProcessing.piInstallationPath ?? '');
  const [piPathState, setPiPathState] = useState<RequestState>('idle');
  const [connectionState, setConnectionState] = useState<ConnectionState>('idle');
  const [connectionMessage, setConnectionMessage] = useState<string | null>(null);
  const [connectionElapsedMs, setConnectionElapsedMs] = useState(0);
  const [destination, setDestination] = useState<Destination | null>(null);
  const [destinationVerified, setDestinationVerified] = useState(false);
  const secretRef = useRef<HTMLInputElement>(null);
  const awsAccessKeyRef = useRef<HTMLInputElement>(null);
  const awsSessionTokenRef = useRef<HTMLInputElement>(null);
  const selectedProviderRef = useRef(selectedId);
  const initialDraftRef = useRef(draft);
  const providerGenerationRef = useRef(0);
  const modelOperationRef = useRef<string | null>(null);
  const connectionOperationRef = useRef<string | null>(null);
  const destinationOperationRef = useRef<string | null>(null);
  const credentialStatusRequestRef = useRef(0);
  const visionOperationRef = useRef<string | null>(null);
  const visionCommitPendingRef = useRef(false);
  const [visionCapability, setVisionCapability] = useState<VisionCapability>('unknown');
  const [manualVisionAllowed, setManualVisionAllowed] = useState(false);
  const [screenPermission, setScreenPermission] = useState<'granted' | 'denied' | 'unknown'>(
    'unknown',
  );
  const [visionDialogOpen, setVisionDialogOpen] = useState(false);
  const [visionNonce, setVisionNonce] = useState('');
  const [visionTestState, setVisionTestState] = useState<RequestState>('idle');
  const [visionCommitPending, setVisionCommitPending] = useState(false);

  const selected = catalog.find((provider) => provider.id === selectedId) ?? null;
  const runnableId = selected !== null && isRunnableProviderId(selected.id) ? selected.id : null;
  const savedDraft = settings.smartProcessing.providers[selectedId] ?? {};
  const dirty = !draftsEqual(draft, savedDraft);
  const endpointDirty = draft.baseUrl !== savedDraft.baseUrl;
  const credentialBindingDirty =
    endpointDirty || (runnableId === 'bedrock' && draft.region !== savedDraft.region);
  const persistedCredentialBinding =
    runnableId === null ? null : credentialBindingKey(runnableId, savedDraft);
  const credentialConfiguredForBinding =
    credentialConfigured && credentialStatusBinding === persistedCredentialBinding;
  const credentialStateForBinding =
    credentialStatusBinding === persistedCredentialBinding ? credentialState : 'loading';
  const configurableEndpoint = selected?.fields.some((field) => field.key === 'baseUrl') ?? false;
  const displayedDestination =
    configurableEndpoint && !destinationVerified
      ? null
      : (destination ?? selected?.destinationHint ?? null);
  async function discoverPiImmediately(
    generation: number,
    currentDraft: ProviderSettingsDraft,
  ): Promise<void> {
    const operationId = nextOperationId('pi', 'models');
    modelOperationRef.current = operationId;
    setModelElapsedMs(0);
    setModelState('loading');
    setModelMessage(null);
    try {
      const discovered = await rendererWatchdog(
        window.talkingQuill.providers.listModels('pi', operationId, false),
        () => window.talkingQuill.providers.cancel(operationId),
      );
      if (
        modelOperationRef.current !== operationId ||
        selectedProviderRef.current !== 'pi' ||
        providerGenerationRef.current !== generation
      )
        return;
      const reconciled = reconcileDiscoveredModels(discovered, currentDraft, true);
      setModels(discovered);
      setModelState(discovered.length === 0 ? 'empty' : 'success');
      setModelMessage(reconciled.message);
      setDraft(reconciled.draft);
    } catch (error: unknown) {
      if (
        modelOperationRef.current === operationId &&
        selectedProviderRef.current === 'pi' &&
        providerGenerationRef.current === generation
      ) {
        setModelState(errorCode(error) === 'CANCELLED' ? 'cancelled' : 'error');
        setModelMessage(piDiscoveryError(error));
      }
    } finally {
      if (modelOperationRef.current === operationId) modelOperationRef.current = null;
    }
  }

  const filtered = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return query.length === 0
      ? catalog
      : catalog.filter((provider) =>
          `${provider.displayName} ${provider.description}`.toLocaleLowerCase().includes(query),
        );
  }, [catalog, search]);

  useEffect(() => {
    if (selectedProviderRef.current !== selectedId) {
      providerGenerationRef.current += 1;
      selectedProviderRef.current = selectedId;
    }
  }, [selectedId]);

  useEffect(() => {
    let active = true;
    void window.talkingQuill.providers
      .catalog()
      .then((providers) => {
        if (!active) return;
        setCatalog(providers);
        setCatalogState(providers.length === 0 ? 'empty' : 'success');
        if (selectedProviderRef.current === 'pi') {
          void discoverPiImmediately(providerGenerationRef.current, initialDraftRef.current);
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
    // Catalog bootstrap intentionally captures the initial Pi draft and discovery function once.
  }, []);

  const refreshCredentialStatus = useCallback(
    async (
      providerId: RunnableProviderId,
      generation: number,
      binding: string,
      requestId: number,
    ): Promise<void> => {
      try {
        const status = await window.talkingQuill.providers.secretStatus(providerId);
        if (
          selectedProviderRef.current !== providerId ||
          providerGenerationRef.current !== generation ||
          credentialStatusRequestRef.current !== requestId
        ) {
          return;
        }
        setCredentialConfigured(status.configured);
        setCredentialStatusBinding(binding);
        setCredentialBindingToken(status.bindingToken);
        setCredentialState('success');
      } catch {
        if (
          selectedProviderRef.current === providerId &&
          providerGenerationRef.current === generation &&
          credentialStatusRequestRef.current === requestId
        ) {
          setCredentialStatusBinding(binding);
          setCredentialBindingToken(null);
          setCredentialState('error');
        }
      }
    },
    [],
  );

  useEffect(() => {
    if (runnableId === null || persistedCredentialBinding === null) return;
    const generation = providerGenerationRef.current;
    const requestId = credentialStatusRequestRef.current + 1;
    credentialStatusRequestRef.current = requestId;
    void refreshCredentialStatus(runnableId, generation, persistedCredentialBinding, requestId);
  }, [persistedCredentialBinding, refreshCredentialStatus, runnableId]);

  useEffect(() => {
    let active = true;
    void window.talkingQuill.providers
      .osaStatus()
      .then((status) => {
        if (!active) return;
        setVisionCapability(status.capability);
        setManualVisionAllowed(status.manualTestAllowed);
        setScreenPermission(status.screenPermission);
      })
      .catch(() => {
        if (active) setVisionCapability('unknown');
      });
    return () => {
      active = false;
    };
  }, [
    settings.smartProcessing.selectedProviderId,
    settings.smartProcessing.providers,
    settings.smartProcessing.credentialEpochs,
    settings.smartProcessing.visionOverrides,
  ]);

  useEffect(
    () => () => {
      for (const operationId of [
        modelOperationRef.current,
        connectionOperationRef.current,
        destinationOperationRef.current,
        visionOperationRef.current,
      ]) {
        if (operationId !== null) void window.talkingQuill.providers.cancel(operationId);
      }
    },
    [],
  );

  const selectProvider = async (provider: ProviderCatalogEntry) => {
    if (!isRunnableProviderId(provider.id)) return;
    const generation = providerGenerationRef.current + 1;
    providerGenerationRef.current = generation;
    selectedProviderRef.current = provider.id;
    setSelectedId(provider.id);
    for (const operationId of [
      modelOperationRef.current,
      connectionOperationRef.current,
      destinationOperationRef.current,
    ]) {
      if (operationId !== null) void window.talkingQuill.providers.cancel(operationId);
    }
    const nextDraft = createDraft(provider, settings.smartProcessing.providers[provider.id]);
    setPickerOpen(false);
    setSearch('');
    setDraft(nextDraft);
    setModels([]);
    setModelState('idle');
    setModelMessage(null);
    setConnectionState('idle');
    setConnectionMessage(null);
    setDestination(null);
    setDestinationVerified(false);
    setFieldErrors({});
    if (provider.id !== selectedId) {
      setCredentialConfigured(false);
      setCredentialStatusBinding(null);
      setCredentialBindingToken(null);
      setCredentialState('loading');
    }
    const config = configFromDraft(provider.id, nextDraft);
    setSaveState('loading');
    try {
      const saved = await window.talkingQuill.providers.saveConfig(config);
      if (
        selectedProviderRef.current === provider.id &&
        providerGenerationRef.current === generation
      ) {
        onSettingsSaved(saved.settings);
        setCredentialConfigured(saved.credentialState.configured);
        setCredentialBindingToken(saved.credentialState.bindingToken);
        setSaveState('success');
      }
    } catch {
      if (
        selectedProviderRef.current === provider.id &&
        providerGenerationRef.current === generation
      ) {
        setSaveState('error');
      }
    }
  };

  const saveConfiguration = async (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => {
    event.preventDefault();
    invalidateOperations();
    if (runnableId === null || selected === null) return;
    const parsed = RunnableProviderConfigSchema.safeParse({ providerId: runnableId, ...draft });
    if (!parsed.success) {
      setFieldErrors(providerFieldErrors(parsed.error.issues));
      setSaveState('error');
      return;
    }
    const generation = providerGenerationRef.current;
    const credentialMustBeReentered = credentialBindingDirty;
    setFieldErrors({});
    setSaveState('loading');
    try {
      const saved = await window.talkingQuill.providers.saveConfig(parsed.data);
      if (
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      onSettingsSaved(saved.settings);
      if (credentialMustBeReentered) {
        setCredentialConfigured(false);
        setCredentialStatusBinding(null);
        setCredentialBindingToken(null);
        setCredentialState('loading');
      } else {
        setCredentialConfigured(saved.credentialState.configured);
        setCredentialBindingToken(saved.credentialState.bindingToken);
      }
      setSaveState('success');
      await verifyDestination(runnableId);
    } catch (error: unknown) {
      if (
        selectedProviderRef.current === runnableId &&
        providerGenerationRef.current === generation
      ) {
        setSaveState('error');
        setConnectionMessage(actionableError(error, runnableId));
      }
    }
  };

  const saveSecret = async () => {
    if (runnableId === null || secretRef.current === null) return;
    invalidateOperations();
    let secret: string;
    if (runnableId === 'bedrock') {
      if (awsAccessKeyRef.current === null || awsSessionTokenRef.current === null) return;
      const accessKeyId = awsAccessKeyRef.current.value;
      const secretAccessKey = secretRef.current.value;
      const sessionToken = awsSessionTokenRef.current.value;
      awsAccessKeyRef.current.value = '';
      secretRef.current.value = '';
      awsSessionTokenRef.current.value = '';
      try {
        secret = serializeAwsCredentials({
          accessKeyId,
          secretAccessKey,
          ...(sessionToken.length === 0 ? {} : { sessionToken }),
        });
      } catch {
        setCredentialState('error');
        return;
      }
    } else {
      secret = secretRef.current.value;
      secretRef.current.value = '';
    }
    const config = RunnableProviderConfigSchema.safeParse({ providerId: runnableId, ...draft });
    if (secret.length === 0 || !config.success) {
      if (!config.success) setFieldErrors(providerFieldErrors(config.error.issues));
      setCredentialState('error');
      return;
    }
    const generation = providerGenerationRef.current;
    setCredentialState('loading');
    try {
      const saved = await window.talkingQuill.providers.saveConfig(config.data);
      if (
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      onSettingsSaved(saved.settings);
      const status = await window.talkingQuill.providers.setSecret(
        runnableId,
        saved.credentialState.bindingToken,
        secret,
      );
      if (
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      setCredentialConfigured(status.configured);
      setCredentialStatusBinding(credentialBindingKey(runnableId, config.data));
      setCredentialBindingToken(status.bindingToken);
      setCredentialState('success');
      await verifyDestination(runnableId);
    } catch {
      if (
        selectedProviderRef.current === runnableId &&
        providerGenerationRef.current === generation
      ) {
        setCredentialState('error');
      }
    }
  };

  const deleteSecret = async () => {
    if (runnableId === null || credentialBindingToken === null) return;
    invalidateOperations();
    const generation = providerGenerationRef.current;
    setCredentialState('loading');
    try {
      const status = await window.talkingQuill.providers.deleteSecret(
        runnableId,
        credentialBindingToken,
      );
      if (
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      setCredentialConfigured(status.configured);
      setCredentialStatusBinding(persistedCredentialBinding);
      setCredentialBindingToken(status.bindingToken);
      setCredentialState('success');
      setDestinationVerified(false);
    } catch {
      if (
        selectedProviderRef.current === runnableId &&
        providerGenerationRef.current === generation
      ) {
        setCredentialState('error');
      }
    }
  };

  const discoverModels = async (refresh = true) => {
    if (runnableId === null || (dirty && runnableId !== 'pi')) return;
    const generation = providerGenerationRef.current;
    const operationId = nextOperationId(runnableId, 'models');
    modelOperationRef.current = operationId;
    setModelElapsedMs(0);
    setModelState('loading');
    setModelMessage(null);
    try {
      const discovered = await rendererWatchdog(
        window.talkingQuill.providers.listModels(runnableId, operationId, refresh),
        () => window.talkingQuill.providers.cancel(operationId),
      );
      if (
        modelOperationRef.current !== operationId ||
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      const reconciled = reconcileDiscoveredModels(discovered, draft, runnableId === 'pi');
      setModels(discovered);
      setModelState(discovered.length === 0 ? 'empty' : 'success');
      setModelMessage(reconciled.message);
      setDraft(reconciled.draft);
      await verifyDestination(runnableId);
    } catch (error: unknown) {
      if (
        modelOperationRef.current === operationId &&
        selectedProviderRef.current === runnableId &&
        providerGenerationRef.current === generation
      ) {
        const cancelled = errorCode(error) === 'CANCELLED';
        setModelState(cancelled ? 'cancelled' : 'error');
        setModelMessage(
          cancelled
            ? 'Model discovery cancelled.'
            : runnableId === 'pi'
              ? piDiscoveryError(error)
              : actionableError(error, runnableId),
        );
      }
    } finally {
      if (modelOperationRef.current === operationId) modelOperationRef.current = null;
    }
  };

  const testConnection = async () => {
    if (runnableId === null || dirty || draft.modelId === undefined || draft.modelId === null)
      return;
    const generation = providerGenerationRef.current;
    const operationId = nextOperationId(runnableId, 'test');
    connectionOperationRef.current = operationId;
    setConnectionState('loading');
    setConnectionElapsedMs(0);
    setConnectionMessage(null);
    try {
      const result = await rendererWatchdog(
        window.talkingQuill.providers.testConnection(runnableId, operationId),
        () => window.talkingQuill.providers.cancel(operationId),
      );
      if (
        connectionOperationRef.current !== operationId ||
        selectedProviderRef.current !== runnableId ||
        providerGenerationRef.current !== generation
      ) {
        return;
      }
      setDestination(result.destination);
      setDestinationVerified(true);
      setConnectionState('success');
      setConnectionMessage(
        `Connection verified. ${String(result.modelCount)} compatible ${result.modelCount === 1 ? 'model' : 'models'} reported.`,
      );
    } catch (error: unknown) {
      if (
        connectionOperationRef.current === operationId &&
        selectedProviderRef.current === runnableId &&
        providerGenerationRef.current === generation
      ) {
        const cancelled = errorCode(error) === 'CANCELLED';
        setConnectionState(cancelled ? 'cancelled' : 'error');
        setConnectionMessage(
          cancelled ? 'Connection test cancelled.' : actionableError(error, runnableId),
        );
      }
    } finally {
      if (connectionOperationRef.current === operationId) connectionOperationRef.current = null;
    }
  };

  const verifyDestination = async (providerId: RunnableProviderId) => {
    const generation = providerGenerationRef.current;
    const previousOperationId = destinationOperationRef.current;
    if (previousOperationId !== null) {
      void window.talkingQuill.providers.cancel(previousOperationId);
    }
    const operationId = nextOperationId(providerId, 'destination');
    destinationOperationRef.current = operationId;
    try {
      const verifiedDestination = await window.talkingQuill.providers.destination(
        providerId,
        operationId,
      );
      if (
        destinationOperationRef.current === operationId &&
        selectedProviderRef.current === providerId &&
        providerGenerationRef.current === generation
      ) {
        setDestination(verifiedDestination);
        setDestinationVerified(true);
      }
    } catch {
      if (
        destinationOperationRef.current === operationId &&
        selectedProviderRef.current === providerId &&
        providerGenerationRef.current === generation
      ) {
        setDestinationVerified(false);
      }
    } finally {
      if (destinationOperationRef.current === operationId) {
        destinationOperationRef.current = null;
      }
    }
  };

  const updatePiInstallation = async (action: 'save' | 'browse' | 'automatic') => {
    invalidateOperations();
    setPiPathState('loading');
    try {
      const status = await rendererWatchdog(
        action === 'browse'
          ? window.talkingQuill.providers.browsePiInstallation()
          : window.talkingQuill.providers.savePiInstallation(
              action === 'automatic' ? null : piPathInput,
            ),
      );
      setPiInstallation(status);
      setPiPathInput(status.configuredPath ?? '');
      setPiPathState(status.state === 'ready' ? 'success' : 'error');
      if (status.state === 'ready' && selectedProviderRef.current === 'pi') {
        await discoverPiImmediately(providerGenerationRef.current, draft);
      }
    } catch (error: unknown) {
      const code = errorCode(error);
      setPiInstallation({
        mode: action === 'automatic' ? 'automatic' : 'configured',
        state:
          code === 'PI_INCOMPATIBLE'
            ? 'incompatible'
            : code === 'PI_CONFIG_INVALID'
              ? 'invalid'
              : 'not-found',
        configuredPath: action === 'automatic' ? null : piPathInput,
        path: null,
        version: null,
        source: null,
        errorCode:
          code === 'PI_INCOMPATIBLE' || code === 'PI_CONFIG_INVALID' || code === 'PI_NOT_FOUND'
            ? code
            : 'PI_CONFIG_INVALID',
      });
      setPiPathState('error');
      setModelMessage(piDiscoveryError(error));
    }
  };

  useEffect(() => {
    if (runnableId !== 'pi') return;
    let active = true;
    void rendererWatchdog(window.talkingQuill.providers.piInstallationStatus())
      .then((status) => {
        if (!active) return;
        setPiInstallation(status);
        setPiPathInput(status.configuredPath ?? '');
      })
      .catch(() => {
        if (active) setPiPathState('error');
      });
    return () => {
      active = false;
    };
  }, [runnableId, settings.smartProcessing.piInstallationPath]);

  useEffect(() => {
    if (modelState !== 'loading') return;
    const timer = window.setInterval(() => setModelElapsedMs((elapsed) => elapsed + 250), 250);
    return () => window.clearInterval(timer);
  }, [modelState]);

  useEffect(() => {
    if (connectionState !== 'loading') return;
    const timer = window.setInterval(() => setConnectionElapsedMs((elapsed) => elapsed + 250), 250);
    return () => window.clearInterval(timer);
  }, [connectionState]);

  const discoverModelsRef = useRef(discoverModels);
  useEffect(() => {
    discoverModelsRef.current = discoverModels;
  });
  useEffect(() => {
    if (runnableId !== 'pi' || saveState === 'loading' || modelState !== 'idle') return;
    void discoverModelsRef.current(false);
  }, [modelState, runnableId, saveState]);

  const cancelModelDiscovery = () => {
    const operationId = modelOperationRef.current;
    modelOperationRef.current = null;
    setModelState('cancelled');
    setModelMessage('Model discovery cancelled.');
    if (operationId !== null) {
      void window.talkingQuill.providers.cancel(operationId).catch(() => undefined);
    }
  };

  const cancelConnectionTest = async () => {
    const operationId = connectionOperationRef.current;
    connectionOperationRef.current = null;
    setConnectionState('cancelled');
    setConnectionMessage('Connection test cancelled.');
    if (operationId !== null) await window.talkingQuill.providers.cancel(operationId);
  };

  const invalidateOperations = () => {
    providerGenerationRef.current += 1;
    for (const operationId of [
      modelOperationRef.current,
      connectionOperationRef.current,
      destinationOperationRef.current,
    ]) {
      if (operationId !== null) void window.talkingQuill.providers.cancel(operationId);
    }
    modelOperationRef.current = null;
    connectionOperationRef.current = null;
    destinationOperationRef.current = null;
    setModelState('idle');
    setModelMessage(null);
    setConnectionState('idle');
    setConnectionMessage(null);
    setDestination(null);
    setDestinationVerified(false);
  };

  const updateDraft = (
    key: keyof ProviderSettingsDraft,
    value: ProviderSettingsDraft[keyof ProviderSettingsDraft],
  ) => {
    invalidateOperations();
    setSaveState('idle');
    setDraft((current) => ({ ...current, [key]: value }));
  };

  const updateOsa = async (enabled: boolean) => {
    try {
      onSettingsSaved(await window.talkingQuill.providers.setOnScreenAwareness(enabled));
    } catch {
      setConnectionMessage('On-Screen Awareness could not be enabled for this model.');
    }
  };

  const beginVisionTest = () => {
    const bytes = new Uint8Array(8);
    crypto.getRandomValues(bytes);
    setVisionNonce(
      `ECHO-${[...bytes]
        .map((value) => value.toString(16).padStart(2, '0'))
        .join('')
        .toUpperCase()}`,
    );
    visionCommitPendingRef.current = false;
    setVisionCommitPending(false);
    setVisionTestState('idle');
    setVisionDialogOpen(true);
  };

  const cancelVisionTest = () => {
    if (visionCommitPendingRef.current) return;
    const operationId = visionOperationRef.current;
    visionOperationRef.current = null;
    setVisionDialogOpen(false);
    setVisionTestState('idle');
    if (operationId !== null) void window.talkingQuill.providers.cancel(operationId);
  };

  const verifyVision = async () => {
    let operationId = nextOperationId(selectedId, 'vision');
    visionOperationRef.current = operationId;
    setVisionTestState('loading');
    try {
      const verification = await window.talkingQuill.providers.verifyVision(
        operationId,
        visionNonce,
      );
      if (visionOperationRef.current !== operationId) return;
      visionCommitPendingRef.current = true;
      setVisionCommitPending(true);
      operationId = nextOperationId(selectedId, 'vision-confirm');
      visionOperationRef.current = operationId;
      const saved = await window.talkingQuill.providers.confirmVision(
        operationId,
        verification.verificationId,
      );
      if (visionOperationRef.current !== operationId) return;
      onSettingsSaved(saved);
      setVisionCapability('supported');
      setManualVisionAllowed(false);
      setVisionTestState('success');
    } catch {
      if (visionOperationRef.current === operationId) setVisionTestState('error');
    } finally {
      if (visionOperationRef.current === operationId) {
        visionOperationRef.current = null;
        visionCommitPendingRef.current = false;
        setVisionCommitPending(false);
      }
    }
  };

  const pickerKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (filtered.length === 0) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const direction = event.key === 'ArrowDown' ? 1 : -1;
      setActiveOption((current) => (current + direction + filtered.length) % filtered.length);
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      const provider = filtered[activeOption];
      if (provider !== undefined) void selectProvider(provider);
    }
    if (event.key === 'Escape') setPickerOpen(false);
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
          <div className="provider-picker">
            <span className="provider-picker__label">Provider</span>
            <button
              type="button"
              className="provider-picker__trigger"
              aria-haspopup="listbox"
              aria-expanded={pickerOpen}
              onClick={() => setPickerOpen((open) => !open)}
            >
              <img src={PROVIDER_LOGOS[selected.id]} alt="" />
              <span>
                <strong>{selected.displayName}</strong>
                <small>{selected.description}</small>
              </span>
              <span aria-hidden="true">⌄</span>
            </button>
            {pickerOpen ? (
              <div className="provider-picker__popover">
                <Input
                  label="Search providers"
                  type="search"
                  value={search}
                  onChange={(event) => {
                    setSearch(event.currentTarget.value);
                    setActiveOption(0);
                  }}
                />
                <div
                  className="provider-picker__list"
                  role="listbox"
                  aria-label="Smart processing providers"
                  aria-activedescendant={filtered[activeOption]?.id}
                  tabIndex={0}
                  onKeyDown={pickerKeyDown}
                >
                  {filtered.map((provider, index) => (
                    <button
                      id={provider.id}
                      key={provider.id}
                      type="button"
                      role="option"
                      aria-selected={provider.id === selected.id}
                      className={
                        index === activeOption ? 'provider-option is-active' : 'provider-option'
                      }
                      onMouseEnter={() => setActiveOption(index)}
                      onClick={() => void selectProvider(provider)}
                    >
                      <img src={PROVIDER_LOGOS[provider.id]} alt="" />
                      <span>
                        <strong>{provider.displayName}</strong>
                        <small>{provider.description}</small>
                      </span>
                    </button>
                  ))}
                  {filtered.length === 0 ? (
                    <EmptyState
                      title="No matching providers"
                      description="Try a provider name or clear the search."
                    />
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>

          {runnableId === null ? null : (
            <>
              <div className="provider-summary">
                <Status tone={destinationTone(displayedDestination)}>
                  {destinationLabel(displayedDestination)} —{' '}
                  {destinationVerified ? 'verified' : 'not yet verified'}
                </Status>
                {displayedDestination === 'cloud' || !destinationVerified ? (
                  <p className="cloud-cost-note">
                    Until verified, assume transcript data may leave this device and the provider
                    may charge your account. Talking Quill does not add a fee.
                  </p>
                ) : null}
              </div>

              {runnableId === 'pi' ? (
                <section className="connection-panel" aria-labelledby="pi-installation-heading">
                  <div>
                    <h3 id="pi-installation-heading">Pi installation path</h3>
                    <p>
                      Auto-detect searches PATH, %APPDATA%\\npm, %PNPM_HOME%, and
                      %LOCALAPPDATA%\\pnpm. Example npm global shim: %APPDATA%\\npm\\pi.cmd.
                    </p>
                  </div>
                  <Input
                    label="Pi installation path"
                    value={piPathInput}
                    placeholder="C:\\Users\\you\\AppData\\Roaming\\npm\\pi.cmd"
                    spellCheck={false}
                    disabled={piPathState === 'loading'}
                    hint="Paste an absolute .cmd, .bat, .exe, extensionless executable, symlink, or containing directory. Talking Quill checks CLI capabilities, not package ownership or version."
                    onChange={(event) => {
                      setPiPathInput(event.currentTarget.value);
                      setPiPathState('idle');
                    }}
                  />
                  <div className="provider-actions">
                    <Button
                      variant="secondary"
                      busy={piPathState === 'loading'}
                      disabled={piPathInput.trim().length === 0}
                      onClick={() => void updatePiInstallation('save')}
                    >
                      Save path
                    </Button>
                    <Button
                      variant="secondary"
                      disabled={piPathState === 'loading'}
                      onClick={() => void updatePiInstallation('browse')}
                    >
                      Browse folder…
                    </Button>
                    <Button
                      variant="quiet"
                      disabled={piPathState === 'loading'}
                      onClick={() => void updatePiInstallation('automatic')}
                    >
                      Auto-detect
                    </Button>
                  </div>
                  {piInstallation === null ? (
                    <Status tone="info" live>
                      Locating Pi…
                    </Status>
                  ) : modelState === 'loading' ? (
                    <Status tone="info" live>
                      Reading Pi models… {formatOperationElapsed(modelElapsedMs)}
                    </Status>
                  ) : piInstallation.state === 'ready' ? (
                    <Status tone={modelState === 'error' ? 'error' : 'success'} live>
                      Pi {piInstallation.version} —{' '}
                      {modelState === 'error' ? 'model read failed' : 'ready'} —{' '}
                      {formatOperationElapsed(modelElapsedMs)}
                    </Status>
                  ) : piInstallation.state === 'invalid' ? (
                    <Status tone="error" live>
                      The configured path is stale or invalid. Choose a valid Pi installation or
                      select Auto-detect; Talking Quill will not silently use another Pi.
                    </Status>
                  ) : piInstallation.state === 'incompatible' ? (
                    <Status tone="error" live>
                      This Pi command is missing required CLI capabilities. Update Pi or choose a
                      different compatible executable.
                    </Status>
                  ) : piInstallation.errorCode === 'PI_LAUNCH_FAILED' ? (
                    <Status tone="error" live>
                      Pi could not complete its bounded version and help checks. Retry or choose a
                      different executable.
                    </Status>
                  ) : (
                    <Status tone="warning" live>
                      Pi was not found. Install it with npm install -g
                      @earendil-works/pi-coding-agent, then select Auto-detect.
                    </Status>
                  )}
                </section>
              ) : null}

              <form className="provider-form" onSubmit={(event) => void saveConfiguration(event)}>
                {selected.fields
                  .filter((field) => !field.secret)
                  .map((field) => (
                    <ProviderFieldControl
                      key={field.key}
                      field={field}
                      value={draft[field.key as keyof ProviderSettingsDraft]}
                      models={models}
                      modelState={modelState}
                      modelDiscovery={selected.modelDiscovery}
                      error={fieldErrors[field.key]}
                      controlsDisabled={saveState === 'loading'}
                      operationsDisabled={dirty && runnableId !== 'pi'}
                      onChange={(value) =>
                        updateDraft(field.key as keyof ProviderSettingsDraft, value)
                      }
                      onDiscover={() => void discoverModels(true)}
                      onCancel={() => cancelModelDiscovery()}
                    />
                  ))}
                <div className="provider-actions">
                  <Button type="submit" busy={saveState === 'loading'} disabled={!dirty}>
                    Save configuration
                  </Button>
                  {dirty ? (
                    <Status tone="warning">Save changes before discovery or testing</Status>
                  ) : null}
                  {saveState === 'success' ? <Status tone="success">Draft saved</Status> : null}
                  {saveState === 'error' ? (
                    <Status tone="error">Configuration update failed; status refreshed</Status>
                  ) : null}
                </div>
              </form>

              {selected.fields.some((field) => field.secret) ? (
                <section className="credential-panel" aria-labelledby="provider-credential-heading">
                  <div className="credential-panel__heading">
                    <div>
                      <h3 id="provider-credential-heading">
                        {runnableId === 'bedrock' ? 'AWS credentials' : 'API key'}
                      </h3>
                      <p>
                        Write-only: stored credentials can be replaced or deleted, but never read
                        back.
                      </p>
                    </div>
                    <Status
                      tone={
                        credentialConfiguredForBinding && !credentialBindingDirty
                          ? 'success'
                          : 'neutral'
                      }
                      live
                    >
                      {credentialConfiguredForBinding && !credentialBindingDirty
                        ? 'Configured'
                        : 'Not configured'}
                    </Status>
                  </div>
                  {credentialBindingDirty ? (
                    <p className="operation-message operation-message--error" role="status">
                      Provider destination changed. Save it, then re-enter credentials; old
                      credentials will never be sent to the new destination.
                    </p>
                  ) : null}
                  {runnableId === 'bedrock' ? (
                    <>
                      <Input
                        ref={awsAccessKeyRef}
                        label="AWS access key ID"
                        type="password"
                        minLength={16}
                        autoComplete="off"
                        spellCheck={false}
                        disabled={credentialStateForBinding === 'loading' || dirty}
                      />
                      <Input
                        ref={secretRef}
                        label="AWS secret access key"
                        type="password"
                        minLength={16}
                        autoComplete="off"
                        spellCheck={false}
                        disabled={credentialStateForBinding === 'loading' || dirty}
                      />
                      <Input
                        ref={awsSessionTokenRef}
                        label="AWS session token (optional)"
                        type="password"
                        minLength={16}
                        autoComplete="off"
                        spellCheck={false}
                        disabled={credentialStateForBinding === 'loading' || dirty}
                        hint="All fields are cleared immediately and stored together in the encrypted vault."
                      />
                    </>
                  ) : (
                    <Input
                      ref={secretRef}
                      label={
                        credentialConfiguredForBinding && !credentialBindingDirty
                          ? 'Replacement API key'
                          : 'API key'
                      }
                      type="password"
                      minLength={8}
                      autoComplete="off"
                      spellCheck={false}
                      disabled={credentialStateForBinding === 'loading' || dirty}
                      hint="The input is cleared immediately when submitted."
                    />
                  )}
                  <div className="provider-actions">
                    <Button
                      variant="secondary"
                      busy={credentialStateForBinding === 'loading'}
                      disabled={dirty}
                      onClick={() => void saveSecret()}
                    >
                      {runnableId === 'bedrock'
                        ? credentialConfiguredForBinding && !credentialBindingDirty
                          ? 'Replace AWS credentials'
                          : 'Store AWS credentials'
                        : credentialConfiguredForBinding && !credentialBindingDirty
                          ? 'Replace API key'
                          : 'Store API key'}
                    </Button>
                    {credentialConfiguredForBinding ? (
                      <Button
                        variant="danger"
                        disabled={credentialStateForBinding === 'loading' || dirty}
                        onClick={() => void deleteSecret()}
                      >
                        {runnableId === 'bedrock' ? 'Delete AWS credentials' : 'Delete API key'}
                      </Button>
                    ) : null}
                    {credentialStateForBinding === 'error' ? (
                      <Status tone="error">Credential action failed</Status>
                    ) : null}
                  </div>
                </section>
              ) : null}

              <section className="connection-panel" aria-labelledby="connection-test-heading">
                <div>
                  <h3 id="connection-test-heading">Connection test</h3>
                  <p>
                    Verifies authentication, endpoint safety, the selected model, and availability.
                    For Pi, this sends a minimal fixed prompt to the selected model and may contact
                    or charge its provider.
                  </p>
                </div>
                <div className="provider-actions">
                  <Button
                    busy={connectionState === 'loading'}
                    disabled={dirty || draft.modelId === undefined || draft.modelId === null}
                    onClick={() => void testConnection()}
                  >
                    Test connection
                  </Button>
                  {!dirty && (draft.modelId === undefined || draft.modelId === null) ? (
                    <Status tone="warning">Select and save a model before testing</Status>
                  ) : null}
                  {connectionState === 'loading' ? (
                    <Status tone="info" live>
                      Testing selected model… {formatOperationElapsed(connectionElapsedMs)}
                    </Status>
                  ) : null}
                  {connectionState === 'loading' ? (
                    <Button variant="secondary" onClick={() => void cancelConnectionTest()}>
                      Cancel test
                    </Button>
                  ) : null}
                  {connectionState === 'error' ? (
                    <Button variant="secondary" onClick={() => void testConnection()}>
                      Retry test
                    </Button>
                  ) : null}
                </div>
                {connectionMessage === null ? null : (
                  <p
                    className={`operation-message operation-message--${connectionState}`}
                    role="status"
                    aria-live="polite"
                  >
                    {connectionMessage}
                  </p>
                )}
              </section>

              <section className="connection-panel" aria-labelledby="osa-heading">
                <div>
                  <h3 id="osa-heading">On-Screen Awareness</h3>
                  <p>
                    Opt in to send one screenshot of the focused display with each Smart cleanup.
                    Capture happens only after transcription and voice-command matching. The image
                    is never reused.
                  </p>
                </div>
                {visionCapability === 'supported' ? (
                  <Toggle
                    checked={settings.smartProcessing.onScreenAwarenessEnabled}
                    disabled={dirty || screenPermission === 'denied'}
                    onChange={(event) => void updateOsa(event.currentTarget.checked)}
                    label="Use the focused display for Smart context"
                    hint="The display is downscaled to 1568 px maximum edge and encoded as JPEG quality 80."
                  />
                ) : visionCapability === 'unsupported' ? (
                  <Status tone="neutral">The selected model does not accept images.</Status>
                ) : manualVisionAllowed ? (
                  <div>
                    <Status tone="warning">Vision support is unknown and remains off.</Status>
                    <p>
                      Generic endpoints vary. Only a successful live image-echo test can enable a
                      manual override, bound to this endpoint, credential revision, and model.
                    </p>
                    <Button variant="secondary" disabled={dirty} onClick={beginVisionTest}>
                      Run disclosed image-echo test
                    </Button>
                  </div>
                ) : (
                  <Status tone="neutral">
                    Vision support is unknown; On-Screen Awareness stays off.
                  </Status>
                )}
                {screenPermission === 'denied' ? (
                  <p className="operation-message operation-message--error" role="status">
                    Screen Recording is denied. On macOS, open System Settings → Privacy &amp;
                    Security → Screen Recording, allow Talking Quill, then restart it.
                  </p>
                ) : null}
              </section>

              <Dialog
                open={visionDialogOpen}
                title="Live image-echo verification"
                description="This test captures the code below and sends that one screenshot to the configured provider. No image is retained. Success creates a narrowly bound manual vision override."
                onClose={cancelVisionTest}
                actions={
                  <>
                    <Button
                      variant="secondary"
                      disabled={visionCommitPending}
                      onClick={cancelVisionTest}
                    >
                      {visionCommitPending
                        ? 'Saving verification…'
                        : visionTestState === 'success'
                          ? 'Close'
                          : 'Cancel'}
                    </Button>
                    <Button
                      busy={visionTestState === 'loading'}
                      disabled={visionCommitPending || visionTestState === 'success'}
                      onClick={() => void verifyVision()}
                    >
                      Capture and verify
                    </Button>
                  </>
                }
              >
                <p aria-label="Image echo verification code" className="vision-test-code">
                  {visionNonce}
                </p>
                {visionTestState === 'success' ? (
                  <Status tone="success" live>
                    Verified. The override is bound to this exact configuration.
                  </Status>
                ) : null}
                {visionTestState === 'error' ? (
                  <Status tone="error" live>
                    The model did not echo the visible code exactly. No override was saved.
                  </Status>
                ) : null}
              </Dialog>

              {modelMessage === null ? null : (
                <p className={`operation-message operation-message--${modelState}`} role="status">
                  {modelMessage}
                </p>
              )}
            </>
          )}
        </>
      ) : null}
    </Card>
  );
}

function ProviderFieldControl({
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
  if (field.kind === 'model') {
    const modelValue = typeof value === 'string' ? value : '';
    const selectedOutsideCatalog =
      modelValue.length > 0 && !models.some((model) => model.id === modelValue);
    const showManualEntry = models.length === 0 || manualModelEntry || selectedOutsideCatalog;
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
          <Select
            label={field.label}
            required={field.required}
            value={modelValue}
            error={error}
            disabled={controlsDisabled}
            onChange={(event) => onChange(event.currentTarget.value)}
          >
            <option value="">Select a discovered model</option>
            {models.map((model) => (
              <option key={model.id} value={model.id}>
                {model.name}
              </option>
            ))}
          </Select>
        )}
        {models.length > 0 ? (
          <Button
            variant="quiet"
            disabled={controlsDisabled}
            onClick={() => {
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
              <Status tone="success">{String(models.length)} models found</Status>
            ) : null}
          </div>
        ) : (
          <Status tone="info">
            Deployment discovery requires Azure management-plane credentials.
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

function createDraft(
  provider: ProviderCatalogEntry,
  stored: ProviderSettingsDraft | undefined,
): ProviderSettingsDraft {
  const defaults: Record<string, string | number> = {};
  for (const field of provider.fields) {
    if (field.secret) continue;
    if (field.key === 'modelId' && provider.defaultModel !== null) {
      defaults[field.key] = provider.defaultModel;
    } else if (field.defaultValue !== undefined) defaults[field.key] = field.defaultValue;
    else if (field.key === 'baseUrl' && field.placeholder !== undefined) {
      defaults[field.key] = field.placeholder;
    }
  }
  return ProviderSettingsDraftSchema.parse({ ...defaults, ...stored });
}

function configFromDraft(
  providerId: RunnableProviderId,
  draft: ProviderSettingsDraft,
): RunnableProviderConfig {
  return RunnableProviderConfigSchema.parse({ providerId, ...draft });
}

function credentialBindingKey(
  providerId: RunnableProviderId,
  config: Pick<ProviderSettingsDraft, 'baseUrl' | 'region'>,
): string {
  return `${config.baseUrl ?? 'fixed'}\u0000${providerId === 'bedrock' ? (config.region ?? '') : ''}`;
}

function formatOperationElapsed(milliseconds: number): string {
  return `${(milliseconds / 1_000).toFixed(1)}s`;
}

function draftsEqual(left: ProviderSettingsDraft, right: ProviderSettingsDraft): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const key of keys) {
    const field = key as keyof ProviderSettingsDraft;
    if (left[field] !== right[field]) return false;
  }
  return true;
}

function providerFieldErrors(
  issues: readonly { readonly path: readonly PropertyKey[]; readonly message: string }[],
): Readonly<Record<string, string>> {
  const errors: Record<string, string> = {};
  for (const issue of issues) {
    const field = issue.path.at(-1);
    if (typeof field === 'string' && errors[field] === undefined) errors[field] = issue.message;
  }
  return errors;
}

function nextOperationId(providerId: RunnableProviderId, kind: string): string {
  operationSequence += 1;
  return `${providerId}-${kind}-${Date.now().toString(36)}-${operationSequence.toString(36)}`;
}

function errorCode(error: unknown): string | null {
  if (typeof error !== 'object' || error === null || !('code' in error)) return null;
  return typeof error.code === 'string' ? error.code : null;
}

function piDiscoveryError(error: unknown): string {
  switch (errorCode(error)) {
    case 'CANCELLED':
      return 'Pi model discovery cancelled.';
    case 'AUTHENTICATION_FAILED':
      return 'Pi authentication failed. Sign in to the required provider with Pi, then retry discovery.';
    case 'INVALID_RESPONSE':
      return 'The installed Pi model list was malformed or incompatible. Update Pi, then retry.';
    case 'NO_MODELS':
      return 'Pi returned no models. Authenticate or enter a strict provider/model ID manually.';
    case 'PI_NOT_FOUND':
      return 'Pi was not found. Run npm install -g @earendil-works/pi-coding-agent, then use Auto-detect.';
    case 'PI_CONFIG_INVALID':
      return 'The configured Pi path is stale or invalid. Choose a valid path or explicitly use Auto-detect.';
    case 'PI_INCOMPATIBLE':
      return 'This Pi command is missing required CLI capabilities. Update Pi or choose another executable.';
    case 'PI_LAUNCH_FAILED':
      return 'Pi could not complete its bounded capability check. Retry or choose another executable.';
    default:
      return 'Pi is unavailable. Check the Pi installation path and retry.';
  }
}

function actionableError(error: unknown, providerId?: RunnableProviderId): string {
  const code = errorCode(error);
  if (providerId === 'pi' && code === 'UNAVAILABLE') {
    return 'Pi could not be launched. Check the selected executable, restart Talking Quill, then retry.';
  }
  if (providerId === 'pi' && code === 'MODEL_NOT_FOUND') {
    return 'The selected Pi model disappeared. Open Settings and select another model.';
  }
  return (
    (code === null ? undefined : ACTIONABLE_ERRORS[code]) ??
    'The provider operation failed safely. Retry after checking the configuration.'
  );
}

function destinationLabel(destination: Destination | null): string {
  if (destination === 'local') return 'Local destination';
  if (destination === 'lan') return 'LAN destination';
  if (destination === 'cloud') return 'Cloud destination';
  return 'Destination unknown';
}

function destinationTone(destination: Destination | null): 'success' | 'info' | 'warning' {
  if (destination === 'local') return 'success';
  if (destination === 'lan') return 'info';
  return 'warning';
}
