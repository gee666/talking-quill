export function restoreNodeAbi(firstError, cleanup, restore) {
  let failure = firstError;
  try {
    cleanup();
  } catch (cleanupError) {
    failure ??= cleanupError;
  }
  try {
    restore();
  } catch (restoreError) {
    failure ??= restoreError;
  }
  return failure;
}
