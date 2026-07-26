import { describe, expect, it } from 'vitest';
import { createNativeOwnedTreeRemoval } from '../../app/src/main/data/native-owned-tree-removal';

describe('native owned-tree removal boundary', () => {
  it('rejects unsupported platforms without invoking a native executable', async () => {
    const remove = createNativeOwnedTreeRemoval('missing-helper', 'linux', 250);
    await expect(remove({ path: 'private-data', expectedFileIdentity: '1:2' })).rejects.toThrow(
      'unavailable',
    );
  });

  it.each([0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])(
    'rejects invalid configurable timeout %s',
    (timeoutMs) => {
      expect(() => createNativeOwnedTreeRemoval('helper', 'win32', timeoutMs)).toThrow(
        'timeout is invalid',
      );
    },
  );

  it('validates file identity before attempting native removal', async () => {
    const remove = createNativeOwnedTreeRemoval('missing-helper', 'win32', 250);
    await expect(
      remove({ path: 'private-data', expectedFileIdentity: 'not-an-identity' }),
    ).rejects.toThrow('invalid file identity');
  });
});
