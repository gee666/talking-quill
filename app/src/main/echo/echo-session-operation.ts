import type { EchoAbortReason, PiFallbackCategory } from '../../shared/schemas/echo-session';
import type { EchoSessionContext } from './echo-session-context';

export function abortSession(
  context: EchoSessionContext,
  reason: EchoAbortReason,
  fallbackCategory?: PiFallbackCategory,
): void {
  const rawFallback =
    (reason === 'provider-error' || reason === 'timeout') &&
    context.state.phase === 'processingSmart' &&
    context.state.transcript !== null;
  context.abort?.abort();
  if (rawFallback) context.abort = new AbortController();
  context.dispatch({
    type: 'abort',
    reason,
    ...(fallbackCategory === undefined ? {} : { fallbackCategory }),
  });
}

export function captureIsMissing(context: EchoSessionContext): boolean {
  return context.capture.captureId === null;
}

export function operationSignal(context: EchoSessionContext): AbortSignal {
  return context.abort?.signal ?? AbortSignal.abort();
}
