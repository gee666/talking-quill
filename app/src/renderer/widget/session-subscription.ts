import type { WidgetApi } from '../../shared/bridge/api';
import type { EchoSessionSnapshot } from '../../shared/schemas/echo-session';

/**
 * Subscribes before requesting the initial snapshot so a slow `ready` response
 * cannot overwrite a newer pushed session state.
 */
export function subscribeToWidgetSession(
  source: Pick<WidgetApi, 'ready' | 'onSessionChanged'>,
  listener: (snapshot: EchoSessionSnapshot) => void,
): () => void {
  let active = true;
  let receivedPush = false;
  const unsubscribe = source.onSessionChanged((snapshot) => {
    receivedPush = true;
    if (active) listener(snapshot);
  });
  void source.ready().then(
    (snapshot) => {
      if (active && !receivedPush) listener(snapshot);
    },
    () => {
      // A future pushed snapshot can still recover a renderer whose ready call raced reload.
    },
  );
  return () => {
    active = false;
    unsubscribe();
  };
}
