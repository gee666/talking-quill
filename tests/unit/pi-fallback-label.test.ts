import { describe, expect, it } from 'vitest';
import {
  isWidgetPointerCancelable,
  piFallbackLabel,
  widgetKeyboardGuidance,
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

  it('surfaces errors with an Error badge and the specific message when there is one', () => {
    expect(
      widgetPresentation({
        ...SMART_SESSION,
        phase: 'error',
        abortReason: 'provider-error',
        fallbackCategory: 'pi-timeout',
        message: 'Microphone was disconnected',
      }),
    ).toEqual({
      heading: 'Could not complete',
      badge: 'Error',
      secondary: 'Microphone was disconnected',
    });
  });

  it('falls back to a default sentence when an error carries no message', () => {
    expect(widgetPresentation({ ...SMART_SESSION, phase: 'error', message: null })).toEqual({
      heading: 'Could not complete',
      badge: 'Error',
      secondary: 'Talking Quill encountered an error.',
    });
  });

  it('describes only controls that are active in the current phase', () => {
    expect(widgetKeyboardGuidance({ ...SMART_SESSION, phase: 'recordingQuick' })).not.toContain(
      'click Stop',
    );
    expect(
      widgetKeyboardGuidance({
        ...SMART_SESSION,
        phase: 'recordingExtended',
        dictationMode: 'extended',
      }),
    ).toContain('click Stop or Cancel');
    expect(widgetKeyboardGuidance(SMART_SESSION)).toBe(
      'Press Escape or click Cancel here to stop before text is inserted.',
    );
    expect(widgetKeyboardGuidance({ ...SMART_SESSION, phase: 'restoringClipboard' })).not.toContain(
      'Press Escape',
    );
  });

  it('keeps pointer Cancel available through pre-commit insertion only', () => {
    expect(isWidgetPointerCancelable('transcribing')).toBe(true);
    expect(isWidgetPointerCancelable('processingSmart')).toBe(true);
    expect(isWidgetPointerCancelable('inserting')).toBe(true);
    expect(isWidgetPointerCancelable('restoringClipboard')).toBe(false);
    expect(isWidgetPointerCancelable('completed')).toBe(false);
    expect(isWidgetPointerCancelable('idle')).toBe(false);
  });
});
