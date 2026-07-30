import {
  ProviderConfigSchema,
  RunnableProviderConfigSchema,
  RunnableProviderIdSchema,
  type ProviderConfig,
  type RunnableProviderConfig,
  type RunnableProviderId,
} from '../../shared/schemas/providers';
import type { ProviderSettingsDraft, Settings } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';

export class ProviderConfigService {
  readonly #settings: SettingsStore;
  readonly #smartRevisionListeners = new Set<(revision: number) => void>();
  #smartFingerprint: string;
  #smartRevision = 0;

  constructor(settings: SettingsStore) {
    this.#settings = settings;
    this.#smartFingerprint = smartFingerprint(settings.get());
    settings.subscribe((next) => {
      const fingerprint = smartFingerprint(next);
      if (fingerprint === this.#smartFingerprint) return;
      this.#smartFingerprint = fingerprint;
      this.#smartRevision += 1;
      for (const listener of this.#smartRevisionListeners) listener(this.#smartRevision);
    });
  }

  smartRevision(): number {
    return this.#smartRevision;
  }

  subscribeSmartRevision(listener: (revision: number) => void): () => void {
    this.#smartRevisionListeners.add(listener);
    return () => this.#smartRevisionListeners.delete(listener);
  }

  get(providerId: RunnableProviderId): ProviderConfig {
    const id = RunnableProviderIdSchema.parse(providerId);
    const draft = this.#settings.get().smartProcessing.providers[id] ?? {};
    return ProviderConfigSchema.parse({ providerId: id, ...draft });
  }

  credentialEpoch(providerIdInput: RunnableProviderId): number {
    const providerId = RunnableProviderIdSchema.parse(providerIdInput);
    return this.#settings.get().smartProcessing.credentialEpochs[providerId] ?? 0;
  }

  async advanceCredentialEpoch(
    providerIdInput: RunnableProviderId,
    credentialEpoch: number,
  ): Promise<Settings> {
    const providerId = RunnableProviderIdSchema.parse(providerIdInput);
    const settings = this.#settings.get();
    return this.#settings.update({
      smartProcessing: {
        credentialEpochs: { [providerId]: credentialEpoch },
        ...(settings.smartProcessing.selectedProviderId === providerId
          ? { onScreenAwarenessEnabled: false }
          : {}),
        visionOverrides: settings.smartProcessing.visionOverrides.filter(
          (override) => override.providerId !== providerId,
        ),
      },
    });
  }

  async save(configInput: RunnableProviderConfig, credentialEpoch?: number): Promise<Settings> {
    const config = RunnableProviderConfigSchema.parse(configInput);
    const providerId = RunnableProviderIdSchema.parse(config.providerId);
    const draft: ProviderSettingsDraft = {
      ...(config.baseUrl === undefined ? {} : { baseUrl: config.baseUrl }),
      ...(config.modelId === undefined ? {} : { modelId: config.modelId }),
      ...(config.contextWindow === undefined ? {} : { contextWindow: config.contextWindow }),
      ...(config.maxOutputTokens === undefined ? {} : { maxOutputTokens: config.maxOutputTokens }),
      ...(config.timeoutMs === undefined ? {} : { timeoutMs: config.timeoutMs }),
      ...(config.keepAlive === undefined ? {} : { keepAlive: config.keepAlive }),
      ...(config.region === undefined ? {} : { region: config.region }),
      ...(config.modelType === undefined ? {} : { modelType: config.modelType }),
      ...(config.thinking === undefined ? {} : { thinking: config.thinking }),
    };
    return this.#settings.update({
      smartProcessing: {
        selectedProviderId: providerId,
        providerReplacements: { [providerId]: draft },
        ...(credentialEpoch === undefined
          ? {}
          : { credentialEpochs: { [providerId]: credentialEpoch } }),
        onScreenAwarenessEnabled: false,
        visionOverrides: this.#settings
          .get()
          .smartProcessing.visionOverrides.filter((override) => override.providerId !== providerId),
      },
    });
  }
}

function smartFingerprint(settings: Settings): string {
  // Settings schemas preserve deterministic key order. Include every request-affecting Smart
  // dimension, including credentials by epoch and manual vision consent, in one monotonic domain.
  return JSON.stringify({
    smartProcessing: settings.smartProcessing,
    customVocabulary: settings.customVocabulary,
    voiceCommands: settings.voiceCommands,
  });
}
