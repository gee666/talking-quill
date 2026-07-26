import { describe, expect, it, vi } from 'vitest';
import { SessionOutcomeWriter } from '../../app/src/main/echo/session-outcome-writer';
import { IDLE_ECHO_SESSION, type EchoSessionState } from '../../app/src/main/echo/session-reducer';
import { DEFAULT_GENERAL_PROFILE } from '../../app/src/shared/schemas/dictation-profiles';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';

function smartInsertionTransition(): readonly [EchoSessionState, EchoSessionState] {
  const previous: EchoSessionState = {
    ...IDLE_ECHO_SESSION,
    sessionId: '11111111-1111-4111-8111-111111111111',
    phase: 'inserting',
    dictationMode: 'quick',
    processingMode: 'smart',
    transcript: 'raw transcript',
    finalText: 'polished transcript',
    insertionState: 'pending',
  };
  return [
    previous,
    {
      ...previous,
      phase: 'restoringClipboard',
      insertionState: 'committed',
    },
  ];
}

describe('SessionOutcomeWriter', () => {
  it('writes the frozen Smart outcome synchronously and commits its retained screenshot', () => {
    const record = vi.fn(() => true);
    const commitScreenshot = vi.fn();
    const cleanup = vi.fn();
    const writer = new SessionOutcomeWriter({
      history: { record },
      smart: {
        beginSession: () => ({
          providerId: 'frozen-provider',
          modelId: 'frozen-model',
          prepare: () => Promise.resolve(),
          process: () => Promise.resolve({ text: 'polished transcript', screenshotFilename: null }),
          commitScreenshot,
          cleanup,
        }),
      },
    });
    const settings = structuredClone(DEFAULT_SETTINGS);
    settings.privacy.historyEnabled = true;
    writer.beginSession(settings, DEFAULT_GENERAL_PROFILE, 'smart');
    writer.setScreenshotFilename('frozen-screenshot.webp');
    settings.smartProcessing.selectedProviderId = 'ollama';
    const [previous, next] = smartInsertionTransition();

    writer.observeTransition(previous, next);

    expect(record).toHaveBeenCalledOnce();
    expect(record).toHaveBeenCalledWith({
      kind: 'smart-completed',
      dictationMode: 'quick',
      processingMode: 'smart',
      rawText: 'raw transcript',
      processedText: 'polished transcript',
      providerId: 'frozen-provider',
      modelId: 'frozen-model',
      screenshotFilename: 'frozen-screenshot.webp',
    });
    expect(commitScreenshot).toHaveBeenCalledOnce();
    expect(cleanup).not.toHaveBeenCalled();
  });

  it('keeps in-flight history revocation one-way and cleans Smart artifacts', () => {
    const record = vi.fn(() => true);
    const cleanup = vi.fn();
    const writer = new SessionOutcomeWriter({
      history: { record },
      smart: {
        beginSession: () => ({
          providerId: 'provider',
          modelId: 'model',
          prepare: () => Promise.resolve(),
          process: () => Promise.resolve({ text: 'polished transcript', screenshotFilename: null }),
          commitScreenshot: vi.fn(),
          cleanup,
        }),
      },
    });
    const settings = structuredClone(DEFAULT_SETTINGS);
    settings.privacy.historyEnabled = true;
    writer.beginSession(settings, DEFAULT_GENERAL_PROFILE, 'smart');
    writer.revokeHistory();
    settings.privacy.historyEnabled = true;
    const [previous, next] = smartInsertionTransition();

    writer.observeTransition(previous, next);

    expect(record).not.toHaveBeenCalled();
    expect(cleanup).toHaveBeenCalledOnce();
    expect(writer.smartSession).toBeNull();
  });
});
