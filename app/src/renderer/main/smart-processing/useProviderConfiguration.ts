import { useCallback, useEffect, useRef, useState } from 'react';
import { isRunnableProviderId } from '../../../shared/bridge/api';
import { serializeAwsCredentials } from '../../../shared/schemas/credentials';
import {
  RunnableProviderConfigSchema,
  type ProviderCatalogEntry,
  type RunnableProviderId,
} from '../../../shared/schemas/providers';
import type { ProviderSettingsDraft, Settings } from '../../../shared/schemas/settings';
import {
  actionableError,
  createDraft,
  credentialBindingKey,
  draftsEqual,
  providerFieldErrors,
  requiresEndpointRepair,
  type RequestState,
} from './provider-utils';
import type { ProviderOperationsController } from './useProviderOperations';
import type {
  ProviderLease,
  ProviderSelectionTicket,
  ProviderUiCoordinator,
} from './useProviderUiCoordinator';

interface CredentialStatusSnapshot {
  readonly providerId: RunnableProviderId;
  readonly binding: string;
  readonly epoch: number;
  readonly configured: boolean;
  readonly bindingToken: string | null;
  readonly state: 'success' | 'error';
}

export function useProviderConfiguration({
  settings,
  onSettingsSaved,
  coordinator,
  operations,
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly coordinator: ProviderUiCoordinator;
  readonly operations: ProviderOperationsController;
}) {
  const [selectedId, setSelectedId] = useState(settings.smartProcessing.selectedProviderId);
  const [draft, setDraft] = useState<ProviderSettingsDraft>(
    () => settings.smartProcessing.providers[settings.smartProcessing.selectedProviderId] ?? {},
  );
  const [fieldErrors, setFieldErrors] = useState<Readonly<Record<string, string>>>({});
  const [saveState, setSaveState] = useState<RequestState>('idle');
  const [providerSelectionPending, setProviderSelectionPending] = useState(false);
  const [credentialSnapshot, setCredentialSnapshot] = useState<CredentialStatusSnapshot | null>(
    null,
  );
  const [credentialActionState, setCredentialActionState] = useState<RequestState>('idle');
  const [credentialMutationPending, setCredentialMutationPending] = useState(false);
  const secretRef = useRef<HTMLInputElement>(null);
  const awsAccessKeyRef = useRef<HTMLInputElement>(null);
  const awsSessionTokenRef = useRef<HTMLInputElement>(null);
  const latestSettingsRef = useRef(settings);
  const reconciledSettingsRef = useRef(settings);
  const credentialStatusRequestRef = useRef(0);
  const saveOperationRef = useRef(0);
  const credentialMutationRef = useRef(false);
  const repairSelectionRef = useRef<{
    readonly providerId: RunnableProviderId;
    readonly baselineProviderId: RunnableProviderId;
  } | null>(null);

  useEffect(() => {
    latestSettingsRef.current = settings;
  }, [settings]);

  const savedDraft = settings.smartProcessing.providers[selectedId] ?? {};
  const dirty = !draftsEqual(draft, savedDraft);
  const providerSelectionPersisted = selectedId === settings.smartProcessing.selectedProviderId;
  const endpointRepairRequired = requiresEndpointRepair(selectedId, draft);
  const endpointDirty = draft.baseUrl !== savedDraft.baseUrl;
  const credentialBindingDirty =
    endpointDirty || (selectedId === 'bedrock' && draft.region !== savedDraft.region);
  const persistedCredentialBinding = credentialBindingKey(selectedId, savedDraft);
  const credentialEpoch = settings.smartProcessing.credentialEpochs[selectedId] ?? 0;
  const credentialStatusMatches =
    providerSelectionPersisted &&
    !endpointRepairRequired &&
    credentialSnapshot?.providerId === selectedId &&
    credentialSnapshot.binding === persistedCredentialBinding &&
    credentialSnapshot.epoch === credentialEpoch;
  const credentialConfigured = credentialStatusMatches && credentialSnapshot.configured;
  const credentialState: RequestState =
    credentialActionState === 'loading' || credentialActionState === 'error'
      ? credentialActionState
      : credentialStatusMatches
        ? credentialSnapshot.state
        : 'loading';
  const credentialBindingToken = credentialStatusMatches ? credentialSnapshot.bindingToken : null;

  const refreshCredentialStatus = useCallback(
    async (providerId: RunnableProviderId, binding: string, epoch: number): Promise<void> => {
      const requestId = credentialStatusRequestRef.current + 1;
      credentialStatusRequestRef.current = requestId;
      await Promise.resolve();
      if (
        credentialStatusRequestRef.current !== requestId ||
        !coordinator.isActiveProvider(providerId)
      ) {
        return;
      }
      setCredentialSnapshot(null);
      setCredentialActionState('idle');
      try {
        const status = await window.talkingQuill.providers.secretStatus(providerId);
        if (
          credentialStatusRequestRef.current !== requestId ||
          !coordinator.isActiveProvider(providerId)
        ) {
          return;
        }
        setCredentialSnapshot({
          providerId,
          binding,
          epoch,
          configured: status.configured,
          bindingToken: status.bindingToken,
          state: 'success',
        });
      } catch {
        if (
          credentialStatusRequestRef.current !== requestId ||
          !coordinator.isActiveProvider(providerId)
        ) {
          return;
        }
        setCredentialSnapshot({
          providerId,
          binding,
          epoch,
          configured: false,
          bindingToken: null,
          state: 'error',
        });
      }
    },
    [coordinator],
  );

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void refreshCredentialStatus(selectedId, persistedCredentialBinding, credentialEpoch);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [credentialEpoch, persistedCredentialBinding, refreshCredentialStatus, selectedId]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const previousSettings = reconciledSettingsRef.current;
      if (
        previousSettings === settings ||
        coordinator.hasPendingSelection() ||
        providerSelectionPending ||
        credentialMutationRef.current ||
        saveState === 'loading'
      ) {
        return;
      }
      const authoritativeId = settings.smartProcessing.selectedProviderId;
      const authoritativeDraft = settings.smartProcessing.providers[authoritativeId] ?? {};
      if (authoritativeId !== selectedId) {
        const repairSelection = repairSelectionRef.current;
        if (
          repairSelection?.providerId === selectedId &&
          repairSelection.baselineProviderId === authoritativeId
        ) {
          reconciledSettingsRef.current = settings;
          return;
        }
        repairSelectionRef.current = null;
        const lease = coordinator.adoptProvider(authoritativeId);
        if (lease === null) return;
        reconciledSettingsRef.current = settings;
        credentialStatusRequestRef.current += 1;
        operations.reset();
        setSelectedId(authoritativeId);
        setDraft(authoritativeDraft);
        setFieldErrors({});
        setSaveState('idle');
        setCredentialSnapshot(null);
        setCredentialActionState('idle');
        repairSelectionRef.current = null;
        void refreshCredentialStatus(
          authoritativeId,
          credentialBindingKey(authoritativeId, authoritativeDraft),
          settings.smartProcessing.credentialEpochs[authoritativeId] ?? 0,
        );
        if (authoritativeId === 'pi') {
          void operations.discoverPiImmediately(authoritativeDraft, lease);
        }
        return;
      }

      repairSelectionRef.current = null;
      const previousId = previousSettings.smartProcessing.selectedProviderId;
      const previousDraft = previousSettings.smartProcessing.providers[previousId] ?? {};
      const draftWasClean = previousId === selectedId && draftsEqual(draft, previousDraft);
      reconciledSettingsRef.current = settings;
      if (draftWasClean && !draftsEqual(draft, authoritativeDraft)) {
        operations.invalidate();
        setDraft(authoritativeDraft);
        setFieldErrors({});
        setSaveState('idle');
      }
    }, 0);
    return () => window.clearTimeout(timer);
  }, [
    coordinator,
    draft,
    operations,
    providerSelectionPending,
    refreshCredentialStatus,
    saveState,
    selectedId,
    settings,
  ]);

  const applySelection = useCallback(
    (
      provider: ProviderCatalogEntry,
      authoritative: Settings,
      ticket: ProviderSelectionTicket,
    ): { readonly draft: ProviderSettingsDraft; readonly lease: ProviderLease } | null => {
      const lease = coordinator.commitSelection(ticket);
      if (lease === null) return null;
      const appliedDraft = createDraft(
        provider,
        authoritative.smartProcessing.providers[provider.id],
      );
      setProviderSelectionPending(false);
      setSelectedId(provider.id);
      setDraft(appliedDraft);
      operations.reset();
      setFieldErrors({});
      setCredentialSnapshot(null);
      setCredentialActionState('idle');
      repairSelectionRef.current = null;
      return { draft: appliedDraft, lease };
    },
    [coordinator, operations],
  );

  const selectProvider = useCallback(
    async (provider: ProviderCatalogEntry, externallyBlocked: boolean): Promise<void> => {
      if (
        !isRunnableProviderId(provider.id) ||
        externallyBlocked ||
        credentialMutationRef.current ||
        saveState === 'loading'
      ) {
        return;
      }
      const ticket = coordinator.beginSelection(provider.id);
      if (ticket === null) return;
      repairSelectionRef.current = null;
      credentialStatusRequestRef.current += 1;
      setProviderSelectionPending(true);
      operations.reset();
      const nextDraft = createDraft(provider, settings.smartProcessing.providers[provider.id]);
      const parsedConfig = RunnableProviderConfigSchema.safeParse({
        providerId: provider.id,
        ...nextDraft,
      });
      setSaveState('loading');
      try {
        if (!parsedConfig.success && requiresEndpointRepair(provider.id, nextDraft)) {
          const lease = coordinator.commitSelection(ticket);
          if (lease === null) return;
          setProviderSelectionPending(false);
          setSelectedId(provider.id);
          setDraft(nextDraft);
          setFieldErrors({});
          setCredentialSnapshot(null);
          setCredentialActionState('idle');
          setSaveState('idle');
          repairSelectionRef.current = {
            providerId: provider.id,
            baselineProviderId: settings.smartProcessing.selectedProviderId,
          };
          return;
        }
        if (!parsedConfig.success) throw parsedConfig.error;
        const saved = await window.talkingQuill.providers.saveConfig(parsedConfig.data);
        if (!coordinator.isCurrentSelection(ticket)) return;
        const applied = applySelection(provider, saved.settings, ticket);
        if (applied === null) return;
        onSettingsSaved(saved.settings);
        setCredentialSnapshot(null);
        setCredentialActionState('idle');
        setSaveState('success');
        if (provider.id === 'pi') {
          void operations.discoverPiImmediately(applied.draft, applied.lease);
        }
      } catch {
        if (!coordinator.isCurrentSelection(ticket)) return;
        const observedSettings = latestSettingsRef.current;
        let latest = observedSettings;
        try {
          const bootstrap = (await window.talkingQuill.app.getBootstrap()) as
            { readonly settings: Settings } | undefined;
          if (!coordinator.isCurrentSelection(ticket)) return;
          latest =
            latestSettingsRef.current === observedSettings && bootstrap !== undefined
              ? bootstrap.settings
              : latestSettingsRef.current;
        } catch {
          if (!coordinator.isCurrentSelection(ticket)) return;
          latest = latestSettingsRef.current;
        }
        if (latest.smartProcessing.selectedProviderId === provider.id) {
          const applied = applySelection(provider, latest, ticket);
          if (applied !== null) {
            onSettingsSaved(latest);
            if (provider.id === 'pi') {
              void operations.discoverPiImmediately(applied.draft, applied.lease);
            }
          }
        } else {
          const retainedLease = coordinator.rejectSelection(ticket);
          if (retainedLease === null) return;
          setProviderSelectionPending(false);
          operations.reset();
          setCredentialSnapshot(null);
          setCredentialActionState('idle');
          const authoritativeId = latest.smartProcessing.selectedProviderId;
          const authoritativeDraft = latest.smartProcessing.providers[authoritativeId] ?? {};
          let authoritativeLease = retainedLease;
          if (retainedLease.providerId !== authoritativeId) {
            const adoptedLease = coordinator.adoptProvider(authoritativeId);
            if (adoptedLease === null) return;
            authoritativeLease = adoptedLease;
            setSelectedId(authoritativeId);
            setDraft(authoritativeDraft);
            setFieldErrors({});
          }
          onSettingsSaved(latest);
          void refreshCredentialStatus(
            authoritativeId,
            credentialBindingKey(authoritativeId, authoritativeDraft),
            latest.smartProcessing.credentialEpochs[authoritativeId] ?? 0,
          );
          if (authoritativeId === 'pi') {
            void operations.discoverPiImmediately(authoritativeDraft, authoritativeLease);
          }
        }
        setSaveState('error');
      } finally {
        if (coordinator.isCurrentSelection(ticket)) {
          coordinator.rejectSelection(ticket);
          setProviderSelectionPending(false);
          setSaveState('idle');
        }
      }
    },
    [
      applySelection,
      coordinator,
      onSettingsSaved,
      operations,
      refreshCredentialStatus,
      saveState,
      settings.smartProcessing.providers,
      settings.smartProcessing.selectedProviderId,
    ],
  );

  const updateDraft = useCallback(
    (
      key: keyof ProviderSettingsDraft,
      value: ProviderSettingsDraft[keyof ProviderSettingsDraft],
    ): void => {
      operations.invalidate();
      setSaveState('idle');
      setDraft((current) => ({ ...current, [key]: value }));
    },
    [operations],
  );

  const saveConfiguration = useCallback(
    async (blocked: boolean): Promise<void> => {
      if (endpointRepairRequired) {
        setSaveState('idle');
        return;
      }
      if (
        blocked ||
        coordinator.hasPendingSelection() ||
        credentialMutationRef.current ||
        saveState === 'loading'
      ) {
        return;
      }
      const lease = operations.invalidate();
      const parsed = RunnableProviderConfigSchema.safeParse({ providerId: selectedId, ...draft });
      if (!parsed.success) {
        setFieldErrors(providerFieldErrors(parsed.error.issues));
        setSaveState('error');
        return;
      }
      const operationId = saveOperationRef.current + 1;
      saveOperationRef.current = operationId;
      const credentialMustBeReentered = credentialBindingDirty;
      setFieldErrors({});
      setSaveState('loading');
      try {
        const saved = await window.talkingQuill.providers.saveConfig(parsed.data);
        if (!coordinator.isCurrent(lease) || saveOperationRef.current !== operationId) return;
        repairSelectionRef.current = null;
        onSettingsSaved(saved.settings);
        if (credentialMustBeReentered) {
          setCredentialSnapshot(null);
          setCredentialActionState('idle');
        } else {
          setCredentialSnapshot({
            providerId: selectedId,
            binding: credentialBindingKey(selectedId, parsed.data),
            epoch: saved.settings.smartProcessing.credentialEpochs[selectedId] ?? 0,
            configured: saved.credentialState.configured,
            bindingToken: saved.credentialState.bindingToken,
            state: 'success',
          });
        }
        setSaveState('success');
        await operations.verifyDestination(selectedId, lease);
      } catch (error: unknown) {
        if (coordinator.isCurrent(lease) && saveOperationRef.current === operationId) {
          setSaveState('error');
          operations.setConnectionError(actionableError(error, selectedId));
        }
      } finally {
        if (saveOperationRef.current === operationId && !coordinator.isCurrent(lease)) {
          setSaveState('idle');
        }
      }
    },
    [
      coordinator,
      credentialBindingDirty,
      draft,
      endpointRepairRequired,
      onSettingsSaved,
      operations,
      saveState,
      selectedId,
    ],
  );

  const saveSecret = useCallback(
    async (blocked: boolean): Promise<void> => {
      if (
        blocked ||
        endpointRepairRequired ||
        !providerSelectionPersisted ||
        credentialMutationRef.current ||
        coordinator.hasPendingSelection() ||
        secretRef.current === null ||
        saveState === 'loading'
      ) {
        return;
      }
      const lease = operations.invalidate();
      let secret: string;
      if (selectedId === 'bedrock') {
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
          setCredentialActionState('error');
          return;
        }
      } else {
        secret = secretRef.current.value;
        secretRef.current.value = '';
      }
      const config = RunnableProviderConfigSchema.safeParse({ providerId: selectedId, ...draft });
      if (secret.length === 0 || !config.success) {
        if (!config.success) setFieldErrors(providerFieldErrors(config.error.issues));
        setCredentialActionState('error');
        return;
      }
      const operationId = saveOperationRef.current + 1;
      saveOperationRef.current = operationId;
      credentialMutationRef.current = true;
      setCredentialMutationPending(true);
      setCredentialActionState('loading');
      try {
        const saved = await window.talkingQuill.providers.saveConfig(config.data);
        if (!coordinator.isCurrent(lease) || saveOperationRef.current !== operationId) return;
        onSettingsSaved(saved.settings);
        const status = await window.talkingQuill.providers.setSecret(
          selectedId,
          saved.credentialState.bindingToken,
          secret,
        );
        if (!coordinator.isCurrent(lease) || saveOperationRef.current !== operationId) return;
        setCredentialSnapshot({
          providerId: selectedId,
          binding: credentialBindingKey(selectedId, config.data),
          epoch: credentialEpoch,
          configured: status.configured,
          bindingToken: status.bindingToken,
          state: 'success',
        });
        setCredentialActionState('idle');
        credentialMutationRef.current = false;
        setCredentialMutationPending(false);
        await operations.verifyDestination(selectedId, lease);
      } catch {
        if (coordinator.isCurrent(lease) && saveOperationRef.current === operationId) {
          setCredentialActionState('error');
        }
      } finally {
        if (saveOperationRef.current === operationId) {
          credentialMutationRef.current = false;
          setCredentialMutationPending(false);
        }
      }
    },
    [
      coordinator,
      credentialEpoch,
      draft,
      endpointRepairRequired,
      onSettingsSaved,
      operations,
      providerSelectionPersisted,
      saveState,
      selectedId,
    ],
  );

  const deleteSecret = useCallback(
    async (blocked: boolean): Promise<void> => {
      if (
        blocked ||
        endpointRepairRequired ||
        !providerSelectionPersisted ||
        credentialMutationRef.current ||
        coordinator.hasPendingSelection() ||
        credentialBindingToken === null ||
        saveState === 'loading'
      ) {
        return;
      }
      const lease = operations.invalidate();
      const operationId = saveOperationRef.current + 1;
      saveOperationRef.current = operationId;
      credentialMutationRef.current = true;
      setCredentialMutationPending(true);
      setCredentialActionState('loading');
      try {
        const status = await window.talkingQuill.providers.deleteSecret(
          selectedId,
          credentialBindingToken,
        );
        if (!coordinator.isCurrent(lease) || saveOperationRef.current !== operationId) return;
        setCredentialSnapshot({
          providerId: selectedId,
          binding: persistedCredentialBinding,
          epoch: credentialEpoch,
          configured: status.configured,
          bindingToken: status.bindingToken,
          state: 'success',
        });
        setCredentialActionState('idle');
        operations.clearDestinationVerification();
      } catch {
        if (coordinator.isCurrent(lease) && saveOperationRef.current === operationId) {
          setCredentialActionState('error');
        }
      } finally {
        if (saveOperationRef.current === operationId) {
          credentialMutationRef.current = false;
          setCredentialMutationPending(false);
        }
      }
    },
    [
      coordinator,
      credentialBindingToken,
      credentialEpoch,
      endpointRepairRequired,
      operations,
      persistedCredentialBinding,
      providerSelectionPersisted,
      saveState,
      selectedId,
    ],
  );

  return {
    selectedId,
    draft,
    savedDraft,
    dirty,
    providerSelectionPersisted,
    endpointRepairRequired,
    fieldErrors,
    saveState,
    providerSelectionPending,
    credentialConfigured,
    credentialBindingDirty,
    credentialEpoch,
    persistedCredentialBinding,
    credentialState,
    credentialMutationPending,
    secretRef,
    awsAccessKeyRef,
    awsSessionTokenRef,
    selectProvider,
    updateDraft,
    saveConfiguration,
    saveSecret,
    deleteSecret,
  } as const;
}

export type ProviderConfigurationController = ReturnType<typeof useProviderConfiguration>;
