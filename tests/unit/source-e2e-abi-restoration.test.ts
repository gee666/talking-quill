import { describe, expect, it, vi } from 'vitest';
import { restoreNodeAbi } from '../../scripts/source-e2e-abi-restoration.mjs';

describe('source E2E Node ABI restoration', () => {
  it('attempts restoration when process cleanup throws and retains the cleanup error', () => {
    const cleanupError = new Error('cleanup failed');
    const restore = vi.fn();
    const result = restoreNodeAbi(
      null,
      () => {
        throw cleanupError;
      },
      restore,
    );
    expect(restore).toHaveBeenCalledOnce();
    expect(result).toBe(cleanupError);
  });

  it('retains the first test error even when cleanup and restoration also fail', () => {
    const first = new Error('test failed');
    const result = restoreNodeAbi(
      first,
      () => {
        throw new Error('cleanup failed');
      },
      () => {
        throw new Error('restore failed');
      },
    );
    expect(result).toBe(first);
  });
});
