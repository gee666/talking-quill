import type { EchoSessionContext } from './echo-session-context';
import { clearResetTimer, scheduleTerminalReset } from './echo-session-presentation';

export function teardown(context: EchoSessionContext): Promise<void> {
  if (context.teardownInFlight !== null) return context.teardownInFlight;
  if (context.teardownComplete && context.captureReconciler.captureOffGuaranteed) {
    return Promise.resolve();
  }
  const teardown = performTeardown(context);
  context.teardownInFlight = teardown;
  const clear = () => {
    if (context.teardownInFlight === teardown) context.teardownInFlight = null;
  };
  void teardown.then(clear, clear);
  return teardown;
}

export async function performTeardown(context: EchoSessionContext): Promise<void> {
  try {
    await context.capture.performTeardown(() => clearResetTimer(context));
  } finally {
    if (context.state.phase !== 'completed') context.outcomes.discardSmartSession();
    context.teardownComplete = true;
    scheduleTerminalReset(context);
  }
}
