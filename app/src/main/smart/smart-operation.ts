import type { ProviderConfigService } from '../providers/provider-config-service';
import { ProviderError } from '../providers/errors';

interface RevisionBoundOperation {
  readonly signal: AbortSignal;
  assertActive(): void;
  normalize(error: unknown): unknown;
  dispose(): void;
}

export function revisionBoundOperation(
  configs: ProviderConfigService,
  revision: number,
  callerSignal: AbortSignal,
): RevisionBoundOperation {
  const active = new Set<AbortController>();
  let stale = false;
  const remove = configs.subscribeSmartRevision((next) => {
    if (next === revision) return;
    stale = true;
    for (const controller of active) controller.abort();
  });
  const operation = sessionOperation(configs, revision, callerSignal, active, () => stale);
  return {
    ...operation,
    dispose: () => {
      operation.dispose();
      remove();
    },
  };
}

export function sessionOperation(
  configs: ProviderConfigService,
  revision: number,
  callerSignal: AbortSignal,
  active: Set<AbortController>,
  invalid: () => boolean,
): RevisionBoundOperation {
  const controller = new AbortController();
  const abort = (): void => controller.abort();
  callerSignal.addEventListener('abort', abort, { once: true });
  active.add(controller);
  const stale = (): boolean => invalid() || configs.smartRevision() !== revision;
  return {
    signal: controller.signal,
    assertActive: () => {
      if (callerSignal.aborted) throw new ProviderError('CANCELLED');
      if (stale()) throw new ProviderError('STALE_CONFIG');
      if (controller.signal.aborted) throw new ProviderError('CANCELLED');
    },
    normalize: (error) => {
      if (callerSignal.aborted) return new ProviderError('CANCELLED');
      if (stale()) return new ProviderError('STALE_CONFIG');
      if (controller.signal.aborted) return new ProviderError('CANCELLED');
      return error;
    },
    dispose: () => {
      callerSignal.removeEventListener('abort', abort);
      active.delete(controller);
    },
  };
}
