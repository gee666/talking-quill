import { describe, expect, it } from 'vitest';
import {
  isWidgetPointerCancelable,
  piFallbackLabel,
} from '../../app/src/renderer/widget/fallback-label';
import { PiFallbackCategorySchema } from '../../app/src/shared/schemas/echo-session';

describe('Pi widget fallback labels', () => {
  it('provides a specific safe label for every Pi fallback category', () => {
    for (const category of PiFallbackCategorySchema.options) {
      const label = piFallbackLabel(category);
      expect(label, category).toMatch(/^The raw transcript was used/u);
      expect(label, category).toMatch(/Pi/u);
    }
    expect(piFallbackLabel(null)).toBeNull();
  });

  it('keeps pointer Cancel available while transcription and Pi processing can be aborted', () => {
    expect(isWidgetPointerCancelable('transcribing')).toBe(true);
    expect(isWidgetPointerCancelable('processingSmart')).toBe(true);
    expect(isWidgetPointerCancelable('completed')).toBe(false);
    expect(isWidgetPointerCancelable('idle')).toBe(false);
  });
});
