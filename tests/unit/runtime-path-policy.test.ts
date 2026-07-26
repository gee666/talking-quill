import { win32 } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  isStrictPathChild,
  selectAbsolutePathOverride,
} from '../../app/src/main/app/runtime-path-policy';

const windowsContainment = {
  isAbsolute: (path: string) => win32.isAbsolute(path),
  relative: (from: string, to: string) => win32.relative(from, to),
  sep: win32.sep,
};
const windowsAbsolute = {
  isAbsolute: (path: string) => win32.isAbsolute(path),
  resolve: (path: string) => win32.resolve(path),
};

describe('runtime path policy', () => {
  it.each([
    { candidate: 'C:\\Temp\\probe-1', expected: true },
    { candidate: 'C:\\Temp', expected: false },
    { candidate: 'C:\\outside', expected: false },
    { candidate: 'D:\\probe-1', expected: false },
  ])('strictly contains $candidate inside the temporary root', ({ candidate, expected }) => {
    expect(isStrictPathChild('C:\\Temp', candidate, windowsContainment)).toBe(expected);
  });

  it('uses one validated absolute evidence override for environment and CLI modes', () => {
    expect(selectAbsolutePathOverride('C:\\evidence', null, windowsAbsolute)).toBe('C:\\evidence');
    expect(selectAbsolutePathOverride(undefined, 'D:\\cli-evidence', windowsAbsolute)).toBe(
      'D:\\cli-evidence',
    );
    expect(selectAbsolutePathOverride('', 'D:\\cli-fallback', windowsAbsolute)).toBe(
      'D:\\cli-fallback',
    );
    expect(() => selectAbsolutePathOverride('relative', null, windowsAbsolute)).toThrow(
      'must be absolute',
    );
    expect(() => selectAbsolutePathOverride(undefined, 'relative', windowsAbsolute)).toThrow(
      'must be absolute',
    );
  });
});
