import { describe, expect, it } from 'vitest';
import { nativePasteFailureCategory } from '../../app/src/main/helper/helper-readiness';

describe('native paste failure diagnostics', () => {
  it('only returns fixed categories for recognized native failure lines', () => {
    expect(nativePasteFailureCategory('keyboard-owner paste unavailable: target changed')).toBe(
      'target-changed',
    );
    expect(
      nativePasteFailureCategory(
        'keyboard-owner clipboard validation: expired=false hash_match=false sequence_match=true',
      ),
    ).toBe('clipboard-content-or-sequence');
    for (const category of [
      'foreground-window',
      'foreground-process',
      'focused-control',
      'focused-process',
      'focus-query',
      'focus-changed',
      'caret-window',
      'caret-query',
      'caret-changed',
      'caret-position',
      'input-mode',
    ]) {
      expect(
        nativePasteFailureCategory(`keyboard-owner paste target validation: ${category}`),
      ).toBe(`target-${category}`);
    }
    for (const line of [
      'private text',
      'keyboard-owner paste unavailable: target changed secret',
      'keyboard-owner clipboard validation: expired=secret hash_match=false sequence_match=true',
      'keyboard-owner paste target validation: private document title',
      'keyboard-owner paste target validation: caret-position private text',
      '__proto__',
    ]) {
      expect(nativePasteFailureCategory(line)).toBeNull();
    }
  });
});
