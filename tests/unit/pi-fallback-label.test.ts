import { describe, expect, it } from 'vitest';
import {
  isWidgetPointerCancelable,
  piFallbackLabel,
  widgetPresentation,
} from '../../app/src/renderer/widget/fallback-label';
import {
  PiFallbackCategorySchema,
  type EchoSessionSnapshot,
} from '../../app/src/shared/schemas/echo-session';

const SMART_SESSION: EchoSessionSnapshot = {
  sessionId: '00000000-0000-4000-8000-000000000001',
  phase: 'processingSmart',
  dictationMode: 'quick',
  processingMode: 'smart',
  alternate: false,
  rms: 0,
  elapsedMs: 100,
  transcript: 'raw transcript',
  abortReason: null,
  fallbackCategory: null,
  completion: null,
  message: null,
};

describe('Pi widget fallback labels', () => {
  it('provides a specific safe label for every Pi fallback category', () => {
    for (const category of PiFallbackCategorySchema.options) {
      const label = piFallbackLabel(category);
      expect(label, category).toMatch(/^The raw transcript was used/u);
      expect(label, category).toMatch(/Pi/u);
    }
    expect(piFallbackLabel(null)).toBeNull();
  });

  it('presents an in-progress fallback consistently as raw', () => {
    expect(
      widgetPresentation({
        ...SMART_SESSION,
        phase: 'inserting',
        fallbackCategory: 'pi-unavailable',
        message: 'Falling back to raw',
      }),
    ).toEqual({
      heading: 'Falling back to raw',
      badge: 'Raw',
      secondary:
        'The raw transcript was used. Pi is unavailable; check its installation in Settings.',
    });
  });

  it('presents completed fallbacks as done while retaining actionable guidance', () => {
    expect(
      widgetPresentation({
        ...SMART_SESSION,
        phase: 'completed',
        abortReason: 'provider-error',
        completion: 'inserted',
        message: 'Inserted',
      }),
    ).toEqual({
      heading: 'Done',
      badge: 'Raw',
      secondary: 'The raw transcript was used. Check the provider in Settings.',
    });
  });

  it('does not claim raw fallback success while insertion is being cancelled', () => {
    expect(
      widgetPresentation({
        ...SMART_SESSION,
        phase: 'inserting',
        fallbackCategory: 'pi-timeout',
        message: 'Cancelling insertion',
      }),
    ).toEqual({
      heading: 'Cancelling',
      badge: 'Raw',
      secondary: 'Stopping before text is inserted',
    });
    expect(
      widgetPresentation({
        ...SMART_SESSION,
        phase: 'cancelled',
        fallbackCategory: 'pi-timeout',
        message: 'Cancelling insertion',
      }),
    ).toEqual({ heading: 'Cancelled', badge: 'Smart', secondary: 'Talking Quill' });
  });

  it('keeps pointer Cancel available while transcription and Pi processing can be aborted', () => {
    expect(isWidgetPointerCancelable('transcribing')).toBe(true);
    expect(isWidgetPointerCancelable('processingSmart')).toBe(true);
    expect(isWidgetPointerCancelable('completed')).toBe(false);
    expect(isWidgetPointerCancelable('idle')).toBe(false);
  });
});
