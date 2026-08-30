import type { ShortcutCaptureLeaseId } from '../../../shared/schemas/shortcut-capture';

const pendingLeases = new Set<ShortcutCaptureLeaseId>();
const activeStops = new Map<ShortcutCaptureLeaseId, Promise<void>>();
const inFlightRestorations = new Set<Promise<void>>();

/**
 * Restores one exact main-minted capture lease. The task is registered before its acquisition
 * resolves, so another editor cannot acquire while this lease is still being minted or restored.
 * Failed capabilities remain in this module-level manager across editor unmounts.
 */
export function restoreShortcutCaptureLease(
  acquisition: Promise<ShortcutCaptureLeaseId>,
): Promise<void> {
  const restoration = acquisition.then(
    async (leaseId) => {
      pendingLeases.add(leaseId);
      await stopPendingLease(leaseId);
    },
    () => {
      // Main revokes failed acquisitions before rejecting, so no capability exists to restore.
    },
  );
  inFlightRestorations.add(restoration);
  void restoration.finally(() => inFlightRestorations.delete(restoration)).catch(() => undefined);
  return restoration;
}

/** Waits for the restorations and retries active when a settings save is requested. */
export async function waitForShortcutCaptureRestorations(): Promise<void> {
  await Promise.all([...inFlightRestorations, ...activeStops.values()]);
  if (pendingLeases.size > 0) {
    throw new Error('Shortcut capture restoration is still pending');
  }
}

/** Waits for prior restorations and retries retained capabilities before a new acquisition. */
export async function retryShortcutCaptureRestorations(): Promise<void> {
  // Snapshot once. A caller can be blurred while it waits here, and that adds a restoration whose
  // acquisition depends on this barrier. Waiting for newly added work would create a self-cycle.
  await Promise.allSettled([...inFlightRestorations]);
  await Promise.all([...pendingLeases].map(stopPendingLease));
}

function stopPendingLease(leaseId: ShortcutCaptureLeaseId): Promise<void> {
  const active = activeStops.get(leaseId);
  if (active !== undefined) return active;
  const stop = window.talkingQuill.shortcutCapture.stop(leaseId).then(() => {
    pendingLeases.delete(leaseId);
  });
  activeStops.set(leaseId, stop);
  void stop.finally(() => activeStops.delete(leaseId)).catch(() => undefined);
  return stop;
}

if (typeof window !== 'undefined') {
  window.addEventListener('focus', () => {
    void retryShortcutCaptureRestorations().catch(() => undefined);
  });
}
