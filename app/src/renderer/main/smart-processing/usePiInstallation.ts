import { useCallback, useEffect, useRef, useState } from 'react';
import type { PiInstallationStatus } from '../../../shared/schemas/pi-installation';
import type { ProviderSettingsDraft } from '../../../shared/schemas/settings';
import { rendererWatchdog } from '../provider-watchdog';
import { errorCode, piDiscoveryError, type RequestState } from './provider-utils';
import type { ProviderOperationsController } from './useProviderOperations';
import type { ProviderUiCoordinator } from './useProviderUiCoordinator';

export type PiInstallationAction = 'save' | 'browse' | 'automatic';

export function usePiInstallation({
  active,
  configuredPath,
  draft,
  coordinator,
  operations,
}: {
  readonly active: boolean;
  readonly configuredPath: string | null;
  readonly draft: ProviderSettingsDraft;
  readonly coordinator: ProviderUiCoordinator;
  readonly operations: ProviderOperationsController;
}) {
  const [installation, setInstallation] = useState<PiInstallationStatus | null>(null);
  const [path, setPathValue] = useState(configuredPath ?? '');
  const [pathState, setPathState] = useState<RequestState>('idle');
  const [message, setMessage] = useState<string | null>(null);
  const statusRequestRef = useRef(0);
  const pathEditRevisionRef = useRef(0);
  const mutationRef = useRef(0);
  const mutationPendingRef = useRef(false);

  useEffect(() => {
    if (!active || mutationPendingRef.current) return;
    const requestId = statusRequestRef.current + 1;
    statusRequestRef.current = requestId;
    const pathEditRevision = pathEditRevisionRef.current;
    let mounted = true;
    void rendererWatchdog(window.talkingQuill.providers.piInstallationStatus())
      .then((status) => {
        if (
          !mounted ||
          statusRequestRef.current !== requestId ||
          !coordinator.isActiveProvider('pi')
        ) {
          return;
        }
        setInstallation(status);
        if (pathEditRevisionRef.current === pathEditRevision) {
          setPathValue(status.configuredPath ?? '');
        }
      })
      .catch(() => {
        if (
          mounted &&
          statusRequestRef.current === requestId &&
          coordinator.isActiveProvider('pi')
        ) {
          setPathState('error');
        }
      });
    return () => {
      mounted = false;
    };
  }, [active, configuredPath, coordinator]);

  const setPath = useCallback((nextPath: string): void => {
    pathEditRevisionRef.current += 1;
    setPathValue(nextPath);
    setPathState('idle');
  }, []);

  const clearMessage = useCallback(() => setMessage(null), []);

  const run = useCallback(
    async (action: PiInstallationAction, externallyBlocked: boolean): Promise<void> => {
      if (
        !active ||
        externallyBlocked ||
        mutationPendingRef.current ||
        coordinator.current().providerId !== 'pi'
      ) {
        return;
      }
      const lease = operations.invalidate();
      const mutationId = mutationRef.current + 1;
      mutationRef.current = mutationId;
      mutationPendingRef.current = true;
      statusRequestRef.current += 1;
      let submittedPath = path;
      setPathState('loading');
      setMessage(null);
      try {
        if (action === 'browse') {
          // Human dialog dwell is intentionally unbounded. Only the subsequent validation and save
          // use the renderer watchdog, matching the main-process operation budget.
          const selectedPath = await window.talkingQuill.providers.browsePiInstallation();
          if (
            mutationRef.current !== mutationId ||
            !coordinator.isCurrent(lease) ||
            coordinator.current().providerId !== 'pi'
          ) {
            return;
          }
          if (selectedPath === null) {
            setPathState('idle');
            return;
          }
          submittedPath = selectedPath;
        }
        const status = await rendererWatchdog(
          window.talkingQuill.providers.savePiInstallation(
            action === 'automatic' ? null : submittedPath,
          ),
        );
        if (
          mutationRef.current !== mutationId ||
          !coordinator.isCurrent(lease) ||
          coordinator.current().providerId !== 'pi'
        ) {
          return;
        }
        setInstallation(status);
        setPathValue(status.configuredPath ?? '');
        setPathState(status.state === 'ready' ? 'success' : 'error');
        if (status.state === 'ready') {
          await operations.discoverPiImmediately(draft, lease);
        }
      } catch (error: unknown) {
        if (
          mutationRef.current !== mutationId ||
          !coordinator.isCurrent(lease) ||
          coordinator.current().providerId !== 'pi'
        ) {
          return;
        }
        const code = errorCode(error);
        setInstallation({
          mode: action === 'automatic' ? 'automatic' : 'configured',
          state:
            code === 'PI_INCOMPATIBLE'
              ? 'incompatible'
              : code === 'PI_CONFIG_INVALID'
                ? 'invalid'
                : 'not-found',
          configuredPath: action === 'automatic' ? null : submittedPath,
          path: null,
          version: null,
          source: null,
          errorCode:
            code === 'PI_INCOMPATIBLE' || code === 'PI_CONFIG_INVALID' || code === 'PI_NOT_FOUND'
              ? code
              : 'PI_CONFIG_INVALID',
        });
        setPathState('error');
        setMessage(piDiscoveryError(error));
      } finally {
        if (mutationRef.current === mutationId) {
          mutationPendingRef.current = false;
          if (!coordinator.isCurrent(lease)) setPathState('idle');
        }
      }
    },
    [active, coordinator, draft, operations, path],
  );

  return {
    installation,
    path,
    pathState,
    message,
    setPath,
    clearMessage,
    run,
  } as const;
}

export type PiInstallationController = ReturnType<typeof usePiInstallation>;
