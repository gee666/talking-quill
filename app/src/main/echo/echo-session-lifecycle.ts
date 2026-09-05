import { teardown } from './echo-session-teardown';
import { helperReadinessError } from './echo-session-activation';
import { type EchoSessionContext } from './echo-session-context';
import { clearResetTimer, publish, reportOperationalFailure } from './echo-session-presentation';
import { dispatch } from './echo-session-transitions';

export async function initialize(context: EchoSessionContext): Promise<void> {
  context.initialized = true;
  if (context.helper.readiness.status === 'ready') {
    try {
      await context.profiles.synchronize();
    } catch {
      // Keep startup usable while retaining one retry request. The profile coordinator reports
      // the operational failure and reopens native activation only after a successful retry.
      context.profiles.requestSync();
    }
  } else context.profiles.requestSync();
  const message = helperReadinessError(context.helper.readiness, context.platform);
  if (message !== null) reportOperationalFailure(context, message);
  else publish(context);
}

export function shutdown(context: EchoSessionContext): Promise<void> {
  if (context.shutdownOperation !== null) return context.shutdownOperation;
  let resolveShutdown!: () => void;
  let rejectShutdown!: (error: unknown) => void;
  const operation = new Promise<void>((resolve, reject) => {
    resolveShutdown = resolve;
    rejectShutdown = reject;
  });
  context.shutdownOperation = operation;
  context.disposed = true;
  context.profiles.dispose();
  context.captureReconciler.beginShutdown();
  context.abort?.abort();
  context.activationTest.stop();
  for (const lease of context.shortcutCaptureLeases.values()) lease.removeOnInvalidated();
  context.shortcutCaptureLeases.clear();
  clearResetTimer(context);
  dispatch(context, { type: 'abort', reason: 'shutdown' });
  void (async () => {
    try {
      await drainEffects(context);
      await teardown(context).catch(() => undefined);
    } finally {
      clearResetTimer(context);
      context.removeSettings();
      context.removeHelperReadiness();
      context.removeHelperNotifications();
      context.listeners.clear();
    }
  })().then(resolveShutdown, rejectShutdown);
  return operation;
}

async function drainEffects(context: EchoSessionContext): Promise<void> {
  let drained: Promise<void>;
  do {
    drained = context.effectTail;
    await drained.catch(() => undefined);
  } while (drained !== context.effectTail);
}
