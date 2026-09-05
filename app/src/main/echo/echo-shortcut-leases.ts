import { randomUUID } from 'node:crypto';
import {
  ShortcutCaptureLeaseIdSchema,
  type ShortcutCaptureLeaseId,
} from '../../shared/schemas/shortcut-capture';
import { type EchoSessionContext } from './echo-session-context';

export async function startShortcutCapture(
  context: EchoSessionContext,
  ownerWebContentsId: number,
  onDestroyed: (listener: () => void) => () => void,
): Promise<ShortcutCaptureLeaseId> {
  if (context.state.phase !== 'idle' || context.activationTest.state.active) {
    throw new Error('Shortcut capture is unavailable during an active session or shortcut test');
  }
  const leaseId = ShortcutCaptureLeaseIdSchema.parse(randomUUID());
  const lease = {
    ownerWebContentsId,
    removeOnInvalidated: (): void => undefined,
    releaseStarted: false,
    releaseOperation: null as Promise<void> | null,
  };
  context.shortcutCaptureLeases.set(leaseId, lease);
  try {
    lease.removeOnInvalidated = onDestroyed(() => {
      void releaseShortcutCaptureLease(context, ownerWebContentsId, leaseId).catch(() => {
        // No renderer remains to retry this capability. The coordinator retains its
        // authoritative background sync request, so only the local tombstone can be dropped.
        context.shortcutCaptureLeases.delete(leaseId);
      });
    });
  } catch (error: unknown) {
    context.shortcutCaptureLeases.delete(leaseId);
    throw error;
  }
  // A lifecycle adapter may invalidate synchronously while registering the listener. Never arm
  // an unowned lease after that invalidation won the race.
  if (context.shortcutCaptureLeases.get(leaseId) !== lease || lease.releaseStarted) {
    if (!lease.releaseStarted) lease.removeOnInvalidated();
    throw new Error('Shortcut capture owner is unavailable');
  }
  try {
    await context.profiles.beginShortcutCapture(leaseId);
    return leaseId;
  } catch (error: unknown) {
    // A rejected start cannot return its capability to the renderer. Revoke it here and restore
    // the authoritative activation state rather than waiting for a later renderer lifecycle.
    await releaseShortcutCaptureLease(context, ownerWebContentsId, leaseId).catch(() => undefined);
    context.shortcutCaptureLeases.delete(leaseId);
    throw error;
  }
}

export async function stopShortcutCapture(
  context: EchoSessionContext,
  ownerWebContentsId: number,
  leaseId: ShortcutCaptureLeaseId,
): Promise<void> {
  await releaseShortcutCaptureLease(context, ownerWebContentsId, leaseId);
}

async function releaseShortcutCaptureLease(
  context: EchoSessionContext,
  ownerWebContentsId: number,
  leaseId: ShortcutCaptureLeaseId,
): Promise<void> {
  const lease = context.shortcutCaptureLeases.get(leaseId);
  if (lease?.ownerWebContentsId !== ownerWebContentsId) return;
  if (lease.releaseOperation !== null) return lease.releaseOperation;
  const operation = (async () => {
    if (!lease.releaseStarted) {
      lease.releaseStarted = true;
      await context.profiles.endShortcutCapture(leaseId);
    } else {
      await context.profiles.retryShortcutCaptureRestoration();
    }
    try {
      lease.removeOnInvalidated();
    } catch {
      // Listener disposal cannot outrank restored authoritative activation.
    }
    context.shortcutCaptureLeases.delete(leaseId);
  })();
  lease.releaseOperation = operation;
  try {
    await operation;
  } finally {
    if (
      context.shortcutCaptureLeases.get(leaseId) === lease &&
      lease.releaseOperation === operation
    ) {
      lease.releaseOperation = null;
    }
  }
}
