import { describe, expect, it } from 'vitest';
import { mapSessionHistoryOutcome } from '../../app/src/main/history/session-history-mapper';

const common = {
  createdAt: 1_000,
  dictationMode: 'quick' as const,
  processingMode: 'raw' as const,
  rawText: 'raw words',
};

describe('session history outcome mapping', () => {
  it('maps Raw completion without optional Smart or command data', () => {
    expect(mapSessionHistoryOutcome({ ...common, kind: 'raw-completed' })).toEqual({
      ...common,
      outcome: 'raw-completed',
      processedText: null,
      providerId: null,
      modelId: null,
      fellBack: false,
      errorCategory: null,
      voiceTrigger: null,
      voiceSnippet: null,
      screenshotFilename: null,
    });
  });

  it('keeps raw and processed Smart text distinct', () => {
    expect(
      mapSessionHistoryOutcome({
        ...common,
        processingMode: 'smart',
        kind: 'smart-completed',
        processedText: 'Polished words.',
        providerId: 'ollama',
        modelId: 'qwen',
      }),
    ).toMatchObject({
      outcome: 'smart-completed',
      rawText: 'raw words',
      processedText: 'Polished words.',
      providerId: 'ollama',
      modelId: 'qwen',
      fellBack: false,
    });
  });

  it.each(['provider-error', 'timeout'] as const)('maps redacted Smart fallback %s', (reason) => {
    expect(
      mapSessionHistoryOutcome({
        ...common,
        processingMode: 'smart',
        kind: 'smart-fallback',
        providerId: 'openai',
        modelId: 'model',
        errorCategory: reason,
      }),
    ).toMatchObject({ outcome: 'smart-fallback', fellBack: true, errorCategory: reason });
  });

  it('supports the Task 8 voice-command extension without matching behavior', () => {
    expect(
      mapSessionHistoryOutcome({
        ...common,
        kind: 'voice-command',
        voiceTrigger: 'signature',
        voiceSnippet: 'Kind regards',
      }),
    ).toMatchObject({
      outcome: 'voice-command',
      voiceTrigger: 'signature',
      voiceSnippet: 'Kind regards',
    });
  });

  it('stores only a redacted error category', () => {
    expect(
      mapSessionHistoryOutcome({ ...common, kind: 'error', errorCategory: 'dictation-error' }),
    ).toMatchObject({ outcome: 'error', errorCategory: 'dictation-error' });
  });
});
