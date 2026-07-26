import { useEffect, useRef, useState } from 'react';
import type { RunnableProviderId, VisionCapability } from '../../../shared/schemas/providers';
import type { ProviderSettingsDraft, Settings } from '../../../shared/schemas/settings';
import type { RequestState } from './provider-utils';
import type { ProviderUiCoordinator } from './useProviderUiCoordinator';

export function useOnScreenAwareness({
  settings,
  selectedId,
  savedDraft,
  providerSelectionPersisted,
  savePending,
  credentialMutationPending,
  dirty,
  coordinator,
  onSettingsSaved,
  onError,
}: {
  readonly settings: Settings;
  readonly selectedId: RunnableProviderId;
  readonly savedDraft: ProviderSettingsDraft;
  readonly providerSelectionPersisted: boolean;
  readonly savePending: boolean;
  readonly credentialMutationPending: boolean;
  readonly dirty: boolean;
  readonly coordinator: ProviderUiCoordinator;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly onError: (message: string) => void;
}) {
  const operationRef = useRef<string | null>(null);
  const commitPendingRef = useRef(false);
  const [capability, setCapability] = useState<VisionCapability>('unknown');
  const [statusIdentity, setStatusIdentity] = useState<{
    readonly providerId: string;
    readonly modelId: string | null;
    readonly configurationKey: string;
  } | null>(null);
  const [manualAllowed, setManualAllowed] = useState(false);
  const [screenPermission, setScreenPermission] = useState<'granted' | 'denied' | 'unknown'>(
    'unknown',
  );
  const [dialogOpen, setDialogOpen] = useState(false);
  const [mutationPending, setMutationPending] = useState(false);
  const [nonce, setNonce] = useState('');
  const [testState, setTestState] = useState<RequestState>('idle');
  const [commitPending, setCommitPending] = useState(false);
  const savedModelId = typeof savedDraft.modelId === 'string' ? savedDraft.modelId : null;
  const credentialEpoch = settings.smartProcessing.credentialEpochs[selectedId] ?? 0;
  const configurationKey = [
    selectedId,
    savedModelId ?? '',
    JSON.stringify(savedDraft),
    String(credentialEpoch),
    JSON.stringify(settings.smartProcessing.visionOverrides),
  ].join('\u0000');
  const statusMatchesSelection =
    statusIdentity?.providerId === selectedId &&
    statusIdentity.modelId === savedModelId &&
    statusIdentity.configurationKey === configurationKey;
  const controlsEnabled =
    providerSelectionPersisted &&
    !savePending &&
    !credentialMutationPending &&
    !mutationPending &&
    !dirty &&
    statusMatchesSelection;

  useEffect(() => {
    let active = true;
    const requestConfigurationKey = configurationKey;
    void window.talkingQuill.providers
      .osaStatus()
      .then((status) => {
        if (!active) return;
        setStatusIdentity({
          providerId: status.providerId,
          modelId: status.modelId,
          configurationKey: requestConfigurationKey,
        });
        setCapability(status.capability);
        setManualAllowed(status.manualTestAllowed);
        setScreenPermission(status.screenPermission);
      })
      .catch(() => {
        if (active) setCapability('unknown');
      });
    return () => {
      active = false;
    };
  }, [configurationKey, settings.smartProcessing.selectedProviderId]);

  useEffect(
    () => () => {
      const operationId = operationRef.current;
      operationRef.current = null;
      if (operationId !== null) {
        void window.talkingQuill.providers.cancel(operationId).catch(() => undefined);
      }
    },
    [],
  );

  const update = async (enabled: boolean) => {
    if (!controlsEnabled) return;
    const lease = coordinator.current();
    setMutationPending(true);
    try {
      const saved = await window.talkingQuill.providers.setOnScreenAwareness(enabled);
      if (coordinator.isCurrent(lease)) onSettingsSaved(saved);
    } catch {
      if (coordinator.isCurrent(lease)) {
        onError('On-Screen Awareness could not be updated for this model.');
      }
    } finally {
      setMutationPending(false);
    }
  };

  const beginTest = () => {
    if (!controlsEnabled) return;
    const bytes = new Uint8Array(8);
    crypto.getRandomValues(bytes);
    setNonce(
      `ECHO-${[...bytes]
        .map((value) => value.toString(16).padStart(2, '0'))
        .join('')
        .toUpperCase()}`,
    );
    commitPendingRef.current = false;
    setCommitPending(false);
    setTestState('idle');
    setDialogOpen(true);
  };

  const cancelTest = () => {
    if (commitPendingRef.current) return;
    const operationId = operationRef.current;
    operationRef.current = null;
    setDialogOpen(false);
    setTestState('idle');
    if (operationId !== null) void window.talkingQuill.providers.cancel(operationId);
  };

  const verify = async () => {
    if (!controlsEnabled) return;
    const lease = coordinator.current();
    let operationId = coordinator.createOperationId(selectedId, 'vision');
    operationRef.current = operationId;
    setTestState('loading');
    try {
      const verification = await window.talkingQuill.providers.verifyVision(operationId, nonce);
      if (operationRef.current !== operationId || !coordinator.isCurrent(lease)) return;
      commitPendingRef.current = true;
      setCommitPending(true);
      operationId = coordinator.createOperationId(selectedId, 'vision-confirm');
      operationRef.current = operationId;
      const saved = await window.talkingQuill.providers.confirmVision(
        operationId,
        verification.verificationId,
      );
      if (operationRef.current !== operationId || !coordinator.isCurrent(lease)) return;
      onSettingsSaved(saved);
      setCapability('supported');
      setManualAllowed(false);
      setTestState('success');
    } catch {
      if (operationRef.current === operationId) setTestState('error');
    } finally {
      if (operationRef.current === operationId) {
        operationRef.current = null;
        commitPendingRef.current = false;
        setCommitPending(false);
      }
    }
  };

  return {
    capability,
    manualAllowed,
    screenPermission,
    controlsEnabled,
    mutationPending,
    dialogOpen,
    nonce,
    testState,
    commitPending,
    update,
    beginTest,
    cancelTest,
    verify,
  } as const;
}
