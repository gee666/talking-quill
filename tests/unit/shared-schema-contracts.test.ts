import { describe, expect, it } from 'vitest';
import { invokeRegistry } from '../../app/src/shared/ipc/registry';
import { MicrophoneIdSchema } from '../../app/src/shared/schemas/audio';
import { HistoryCreateSchema } from '../../app/src/shared/schemas/history';
import {
  PROVIDER_IDS,
  PersistedProviderConfigSchema,
  ProviderConfigSchema,
  ProviderModelIdSchema,
} from '../../app/src/shared/schemas/providers';
import {
  PublicSettingsPatchSchema,
  SettingsPatchSchema,
  TranscriptionSettingsSchema,
} from '../../app/src/shared/schemas/settings';
import {
  TranscriptionLanguageSchema,
  TranscriptionOptionsSchema,
} from '../../app/src/shared/schemas/transcription';
import { MicrophoneEvidenceSchema } from '../../app/src/shared/schemas/welcome';

describe('shared schema invariants', () => {
  it('uses one auto-detect or supported source-language contract at persistence and runtime', () => {
    const modelId = 'onnx-community/whisper-large-v3-turbo';
    for (const language of ['auto', 'en', 'zh', 'ru', 'haw', 'Mandarin', ' x ', '', null]) {
      const accepted = TranscriptionLanguageSchema.safeParse(language).success;
      expect(TranscriptionSettingsSchema.safeParse({ modelId, language }).success).toBe(accepted);
      expect(TranscriptionOptionsSchema.safeParse({ modelId, language }).success).toBe(accepted);
    }
    expect(TranscriptionOptionsSchema.safeParse({ modelId }).success).toBe(false);
    expect(
      TranscriptionOptionsSchema.safeParse({ modelId, language: 'ru', task: 'translate' }).success,
    ).toBe(false);
  });

  it('uses the microphone ID contract at settings, capture, and welcome boundaries', () => {
    const longestId = 'm'.repeat(1_024);
    expect(MicrophoneIdSchema.safeParse(longestId).success).toBe(true);
    expect(
      MicrophoneEvidenceSchema.safeParse({
        boundDeviceId: longestId,
        observedRms: 0.5,
        usableThreshold: 0.1,
        sampleCount: 10,
        observedAt: 1,
      }).success,
    ).toBe(true);
    for (const invalid of ['', 'm'.repeat(1_025)]) {
      expect(MicrophoneIdSchema.safeParse(invalid).success, JSON.stringify(invalid)).toBe(false);
    }
  });

  it('accepts only transport-supported endpoints for new configuration without breaking v19 data', () => {
    expect(
      ProviderConfigSchema.safeParse({ providerId: 'ollama', baseUrl: 'http://127.0.0.1:11434' })
        .success,
    ).toBe(true);
    for (const baseUrl of ['file:///legacy/provider', 'ftp://legacy.example/models']) {
      expect(
        PersistedProviderConfigSchema.safeParse({ providerId: 'ollama', baseUrl }).success,
        baseUrl,
      ).toBe(true);
      for (const field of ['providers', 'providerReplacements'] as const) {
        expect(
          SettingsPatchSchema.safeParse({
            smartProcessing: { [field]: { ollama: { baseUrl } } },
          }).success,
          `${field}: ${baseUrl}`,
        ).toBe(false);
      }
      expect(
        invokeRegistry['provider:config-save'].request.safeParse({
          config: { providerId: 'ollama', baseUrl },
        }).success,
        `provider:config-save: ${baseUrl}`,
      ).toBe(false);
    }
    for (const baseUrl of [
      'not a URL',
      'http://',
      'file:///tmp/provider',
      'ftp://provider.example/models',
      'https://user:secret@provider.example',
      'https://provider.example?token=secret',
      'https://provider.example/#models',
    ]) {
      expect(
        () => ProviderConfigSchema.safeParse({ providerId: 'ollama', baseUrl }),
        baseUrl,
      ).not.toThrow();
      expect(
        ProviderConfigSchema.safeParse({ providerId: 'ollama', baseUrl }).success,
        baseUrl,
      ).toBe(false);
    }
  });

  it('keeps valid provider model IDs valid in history', () => {
    const modelId = 'm'.repeat(512);
    expect(ProviderModelIdSchema.safeParse(modelId).success).toBe(true);
    expect(
      HistoryCreateSchema.safeParse({
        dictationMode: 'quick',
        processingMode: 'smart',
        outcome: 'smart-completed',
        rawText: 'raw',
        processedText: 'processed',
        providerId: 'ollama',
        modelId,
        fellBack: false,
        errorCategory: null,
        voiceTrigger: null,
        voiceSnippet: null,
        screenshotFilename: null,
      }).success,
    ).toBe(true);
  });

  it('rejects recursively empty settings patches but permits explicit empty-list replacements', () => {
    for (const patch of [
      {},
      { app: {} },
      { privacy: {} },
      { smartProcessing: {} },
      { smartProcessing: { providers: {} } },
      { welcome: {} },
    ]) {
      expect(SettingsPatchSchema.safeParse(patch).success, JSON.stringify(patch)).toBe(false);
    }
    expect(PublicSettingsPatchSchema.safeParse({ recording: {} }).success).toBe(false);
    expect(SettingsPatchSchema.safeParse({ voiceCommands: [] }).success).toBe(true);
    expect(
      SettingsPatchSchema.safeParse({
        smartProcessing: { providerReplacements: { openai: {} } },
      }).success,
    ).toBe(true);
  });

  it('keeps Pi browse as a path-only dialog result before bounded save validation', () => {
    const browseResponse = invokeRegistry['provider:pi-installation-browse'].response;
    expect(browseResponse.safeParse({ path: null }).success).toBe(true);
    expect(browseResponse.safeParse({ path: 'C:\\Tools\\pi.cmd' }).success).toBe(true);
    expect(
      browseResponse.safeParse({
        mode: 'configured',
        state: 'ready',
        configuredPath: 'C:\\Tools\\pi.cmd',
      }).success,
    ).toBe(false);
  });

  it('requires the provider catalog to match the canonical inventory and order', () => {
    const providers = PROVIDER_IDS.map((id) => ({
      id,
      displayName: id,
      description: `${id} provider`,
      logo: `${id}.png`,
      destinationHint: 'cloud' as const,
      defaultModel: null,
      modelDiscovery: 'remote' as const,
      fields: [],
    }));
    const schema = invokeRegistry['provider:catalog'].response;
    expect(schema.safeParse({ providers }).success).toBe(true);
    expect(
      schema.safeParse({ providers: [providers[1], providers[0], ...providers.slice(2)] }).success,
    ).toBe(false);
  });
});
