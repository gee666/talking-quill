/**
 * Automatic model discovery must run at most once per *configuration*, never once per mount.
 *
 * The settings screen remounts the Smart processing section on every section switch, and any draft
 * edit bumps the provider lease generation, so per-component state would re-arm discovery after a
 * navigation away and back or after an edit-then-revert. Keeping the memory at module scope makes
 * both of those inert while a genuine configuration change (different provider, endpoint, region or
 * credential epoch) still produces a new key and therefore one fresh attempt.
 */
const attempted = new Set<string>();

/** Identity of the persisted configuration a discovery attempt would be made against. */
export function autoDiscoveryKey(
  providerId: string,
  credentialBinding: string,
  credentialEpoch: number,
): string {
  return `${providerId}\u0000${credentialBinding}\u0000${String(credentialEpoch)}`;
}

/** Returns `true` exactly once per key: the caller may then start one discovery. */
export function claimAutoDiscovery(key: string): boolean {
  if (attempted.has(key)) return false;
  attempted.add(key);
  return true;
}

/** Test seam. The memory intentionally outlives every component instance. */
export function resetAutoDiscoveryMemory(): void {
  attempted.clear();
}
