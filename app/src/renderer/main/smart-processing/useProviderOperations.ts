import { useCallback, useEffect, useRef, useState } from 'react';
import type { Destination, ModelInfo, RunnableProviderId } from '../../../shared/schemas/providers';
import type { ProviderSettingsDraft } from '../../../shared/schemas/settings';
import { rendererWatchdog } from '../provider-watchdog';
import { reconcileDiscoveredModels } from '../pi-model-selection';
import {
  actionableError,
  errorCode,
  piDiscoveryError,
  type ConnectionState,
  type RequestState,
} from './provider-utils';
import type { ProviderLease, ProviderUiCoordinator } from './useProviderUiCoordinator';

export function useProviderOperations(coordinator: ProviderUiCoordinator) {
  const [models, setModels] = useState<readonly ModelInfo[]>([]);
  const [modelState, setModelState] = useState<RequestState>('idle');
  const [modelMessage, setModelMessage] = useState<string | null>(null);
  const [modelElapsedMs, setModelElapsedMs] = useState(0);
  const [connectionState, setConnectionState] = useState<ConnectionState>('idle');
  const [connectionMessage, setConnectionMessage] = useState<string | null>(null);
  const [connectionElapsedMs, setConnectionElapsedMs] = useState(0);
  const [destination, setDestination] = useState<Destination | null>(null);
  const [destinationVerified, setDestinationVerified] = useState(false);
  const destinationAuthorityRef = useRef(0);

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

  const resetOperationState = useCallback(() => {
    destinationAuthorityRef.current += 1;
    setModelState('idle');
    setModelMessage(null);
    setConnectionState('idle');
    setConnectionMessage(null);
    setDestination(null);
    setDestinationVerified(false);
  }, []);

  const reset = useCallback(() => {
    setModels([]);
    resetOperationState();
  }, [resetOperationState]);

  const invalidate = useCallback((): ProviderLease => {
    const lease = coordinator.invalidate();
    resetOperationState();
    return lease;
  }, [coordinator, resetOperationState]);

  const verifyDestination = useCallback(
    async (providerId: RunnableProviderId, expectedLease?: ProviderLease): Promise<void> => {
      if (expectedLease !== undefined && !coordinator.isCurrent(expectedLease)) return;
      const ticket = coordinator.beginOperation('destination', providerId, 'destination');
      if (ticket === null) return;
      const destinationAuthority = destinationAuthorityRef.current + 1;
      destinationAuthorityRef.current = destinationAuthority;
      try {
        const verifiedDestination = await window.talkingQuill.providers.destination(
          providerId,
          ticket.operationId,
        );
        if (
          !coordinator.isCurrentOperation(ticket) ||
          destinationAuthorityRef.current !== destinationAuthority
        ) {
          return;
        }
        setDestination(verifiedDestination);
        setDestinationVerified(true);
      } catch {
        if (
          coordinator.isCurrentOperation(ticket) &&
          destinationAuthorityRef.current === destinationAuthority
        ) {
          setDestinationVerified(false);
        }
      } finally {
        coordinator.finishOperation(ticket);
      }
    },
    [coordinator],
  );

  const runModelDiscovery = useCallback(
    async ({
      providerId,
      draft,
      refresh,
      verifyAfter,
      expectedLease,
    }: {
      readonly providerId: RunnableProviderId;
      readonly draft: ProviderSettingsDraft;
      readonly refresh: boolean;
      readonly verifyAfter: boolean;
      readonly expectedLease?: ProviderLease;
    }): Promise<void> => {
      if (expectedLease !== undefined && !coordinator.isCurrent(expectedLease)) return;
      const ticket = coordinator.beginOperation('models', providerId, 'models');
      if (ticket === null) return;
      setModelElapsedMs(0);
      setModelState('loading');
      setModelMessage(null);
      try {
        const discovered = await rendererWatchdog(
          window.talkingQuill.providers.listModels(providerId, ticket.operationId, refresh),
          () => window.talkingQuill.providers.cancel(ticket.operationId),
        );
        if (!coordinator.isCurrentOperation(ticket)) return;
        const reconciled = reconcileDiscoveredModels(discovered, draft, providerId === 'pi');
        setModels(discovered);
        setModelState(discovered.length === 0 ? 'empty' : 'success');
        setModelMessage(reconciled.message);
        if (verifyAfter) await verifyDestination(providerId, ticket);
      } catch (error: unknown) {
        if (!coordinator.isCurrentOperation(ticket)) return;
        const cancelled = errorCode(error) === 'CANCELLED';
        setModelState(cancelled ? 'cancelled' : 'error');
        setModelMessage(
          cancelled
            ? providerId === 'pi'
              ? piDiscoveryError(error)
              : 'Model discovery cancelled.'
            : providerId === 'pi'
              ? piDiscoveryError(error)
              : actionableError(error, providerId),
        );
      } finally {
        coordinator.finishOperation(ticket);
      }
    },
    [coordinator, verifyDestination],
  );

  const discoverModels = useCallback(
    async ({
      providerId,
      draft,
      refresh = true,
      configurationDirty,
    }: {
      readonly providerId: RunnableProviderId;
      readonly draft: ProviderSettingsDraft;
      readonly refresh?: boolean;
      readonly configurationDirty: boolean;
    }): Promise<void> => {
      if (configurationDirty && providerId !== 'pi') return;
      await runModelDiscovery({
        providerId,
        draft,
        refresh,
        verifyAfter: true,
      });
    },
    [runModelDiscovery],
  );

  const discoverPiImmediately = useCallback(
    async (draft: ProviderSettingsDraft, expectedLease?: ProviderLease): Promise<void> => {
      await runModelDiscovery({
        providerId: 'pi',
        draft,
        refresh: false,
        verifyAfter: false,
        ...(expectedLease === undefined ? {} : { expectedLease }),
      });
    },
    [runModelDiscovery],
  );

  const testConnection = useCallback(
    async ({
      providerId,
      blocked,
    }: {
      readonly providerId: RunnableProviderId;
      readonly blocked: boolean;
    }): Promise<void> => {
      if (blocked) return;
      coordinator.cancelOperation('destination');
      const ticket = coordinator.beginOperation('connection', providerId, 'test');
      if (ticket === null) return;
      const destinationAuthority = destinationAuthorityRef.current + 1;
      destinationAuthorityRef.current = destinationAuthority;
      setConnectionElapsedMs(0);
      setConnectionState('loading');
      setConnectionMessage(null);
      try {
        const result = await rendererWatchdog(
          window.talkingQuill.providers.testConnection(providerId, ticket.operationId),
          () => window.talkingQuill.providers.cancel(ticket.operationId),
        );
        if (!coordinator.isCurrentOperation(ticket)) return;
        if (destinationAuthorityRef.current === destinationAuthority) {
          setDestination(result.destination);
          setDestinationVerified(true);
        }
        setConnectionState('success');
        setConnectionMessage(
          `Connection verified. ${String(result.modelCount)} compatible ${result.modelCount === 1 ? 'model' : 'models'} reported.`,
        );
      } catch (error: unknown) {
        if (!coordinator.isCurrentOperation(ticket)) return;
        const cancelled = errorCode(error) === 'CANCELLED';
        setConnectionState(cancelled ? 'cancelled' : 'error');
        setConnectionMessage(
          cancelled ? 'Connection test cancelled.' : actionableError(error, providerId),
        );
      } finally {
        coordinator.finishOperation(ticket);
      }
    },
    [coordinator],
  );

  const setConnectionError = useCallback((message: string) => {
    setConnectionMessage(message);
  }, []);

  const clearDestinationVerification = useCallback(() => {
    destinationAuthorityRef.current += 1;
    setDestinationVerified(false);
  }, []);

  const cancelModelDiscovery = useCallback(() => {
    if (coordinator.cancelOperation('models') === null) return;
    setModelState('cancelled');
    setModelMessage('Model discovery cancelled.');
  }, [coordinator]);

  const cancelConnectionTest = useCallback((): void => {
    if (coordinator.cancelOperation('connection') === null) return;
    setConnectionState('cancelled');
    setConnectionMessage('Connection test cancelled.');
  }, [coordinator]);

  return {
    models,
    modelState,
    modelMessage,
    modelElapsedMs,
    connectionState,
    connectionMessage,
    connectionElapsedMs,
    destination,
    destinationVerified,
    reset,
    invalidate,
    discoverModels,
    discoverPiImmediately,
    testConnection,
    verifyDestination,
    setConnectionError,
    clearDestinationVerification,
    cancelModelDiscovery,
    cancelConnectionTest,
  } as const;
}

export type ProviderOperationsController = ReturnType<typeof useProviderOperations>;
