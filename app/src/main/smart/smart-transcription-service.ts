import { randomUUID } from 'node:crypto';
import type { HelperClient } from '../helper';
import type { SettingsStore } from '../persistence/settings-store';
import type { ProviderConfigService } from '../providers/provider-config-service';
import type { ProviderService } from '../providers/provider-service';
import { ProviderError } from '../providers/errors';
import { resolveVisionCapability } from '../providers/vision-capabilities';
import type { CapturedScreenshot, ScreenshotService } from '../screenshot/screenshot-service';
import { MANUAL_VISION_PROVIDER_IDS } from '../providers/vision-capabilities';
import type {
  RunnableProviderId,
  VisionCapability,
  VisionVerification,
} from '../../shared/schemas/providers';
import { RetainedScreenshot } from '../screenshot/screenshot-retention';
import { normalizeSmartOutput } from './output-processing';
import {
  DEFAULT_GENERAL_PROFILE,
  type DictationProfile,
} from '../../shared/schemas/dictation-profiles';

import { beginSmartSession } from './smart-session';
import { revisionBoundOperation } from './smart-operation';
import type {
  FrozenSmartTranscriptSession,
  SmartTranscriptProcessor,
  RetainedScreenshotHandle,
} from './smart-session-types';

export type {
  SmartProcessingResult,
  FrozenSmartTranscriptSession,
  SmartTranscriptProcessor,
} from './smart-session-types';

export class SmartTranscriptionService implements SmartTranscriptProcessor {
  readonly #settings: SettingsStore;
  readonly #configs: ProviderConfigService;
  readonly #providers: ProviderService;
  readonly #screenshots: Pick<ScreenshotService, 'capture' | 'permissionStatus'>;
  readonly #helper: Pick<HelperClient, 'getFrontApp'>;
  readonly #screenshotsDirectory: string;
  readonly #retainScreenshot: (
    directory: string,
    screenshot: CapturedScreenshot,
  ) => RetainedScreenshotHandle;
  readonly #pendingVision = new Map<
    string,
    {
      readonly providerId: 'generic-openai' | 'litellm';
      readonly modelId: string;
      readonly binding: string;
      readonly revision: number;
      readonly expiresAt: number;
    }
  >();

  constructor(options: {
    readonly settings: SettingsStore;
    readonly configs: ProviderConfigService;
    readonly providers: ProviderService;
    readonly screenshots: Pick<ScreenshotService, 'capture' | 'permissionStatus'>;
    readonly helper: Pick<HelperClient, 'getFrontApp'>;
    readonly screenshotsDirectory: string;
    readonly retainScreenshot?: (
      directory: string,
      screenshot: CapturedScreenshot,
    ) => RetainedScreenshotHandle;
  }) {
    this.#settings = options.settings;
    this.#configs = options.configs;
    this.#providers = options.providers;
    this.#screenshots = options.screenshots;
    this.#helper = options.helper;
    this.#screenshotsDirectory = options.screenshotsDirectory;
    this.#retainScreenshot =
      options.retainScreenshot ??
      ((directory, screenshot) => new RetainedScreenshot(directory, screenshot));
  }

  status(): {
    readonly providerId: RunnableProviderId;
    readonly modelId: string | null;
    readonly capability: VisionCapability;
    readonly manualTestAllowed: boolean;
    readonly screenPermission: 'granted' | 'denied' | 'unknown';
  } {
    const settings = this.#settings.get();
    const providerId = settings.smartProcessing.selectedProviderId;
    const config = this.#configs.get(providerId);
    const modelId = config.modelId ?? null;
    const binding = this.#visionBinding(config, settings, providerId);
    const capability =
      modelId === null
        ? 'unknown'
        : resolveVisionCapability({
            providerCapability: this.#providers.capabilities(config, modelId),
            providerId,
            modelId,
            binding,
            overrides: settings.smartProcessing.visionOverrides,
          });
    return Object.freeze({
      providerId,
      modelId,
      capability,
      manualTestAllowed:
        capability === 'unknown' &&
        MANUAL_VISION_PROVIDER_IDS.includes(providerId as 'generic-openai' | 'litellm'),
      screenPermission: this.#screenshots.permissionStatus(),
    });
  }

  async setOnScreenAwareness(enabled: boolean): Promise<ReturnType<SettingsStore['get']>> {
    if (!enabled) {
      return this.#settings.update({
        smartProcessing: { onScreenAwarenessEnabled: false },
      });
    }
    const revision = this.#configs.smartRevision();
    const operation = revisionBoundOperation(this.#configs, revision, new AbortController().signal);
    try {
      const settings = this.#settings.get();
      const providerId = settings.smartProcessing.selectedProviderId;
      const config = this.#configs.get(providerId);
      const modelId = config.modelId;
      if (modelId === undefined || modelId === null) throw new ProviderError('INVALID_CONFIG');
      const binding = this.#visionBinding(config, settings, providerId);
      const capability = resolveVisionCapability({
        providerCapability: await this.#providers.preflightCapability(
          config,
          modelId,
          operation.signal,
        ),
        providerId,
        modelId,
        binding,
        overrides: settings.smartProcessing.visionOverrides,
      });
      operation.assertActive();
      if (capability !== 'supported') throw new ProviderError('INVALID_CONFIG');
      return await this.#settings.update(
        { smartProcessing: { onScreenAwarenessEnabled: true } },
        operation.signal,
      );
    } catch (error: unknown) {
      throw operation.normalize(error);
    } finally {
      operation.dispose();
    }
  }

  async verifyManualVision(nonce: string, signal: AbortSignal): Promise<VisionVerification> {
    const revision = this.#configs.smartRevision();
    const status = this.status();
    if (!status.manualTestAllowed || status.modelId === null || !/^[A-Z0-9-]{8,48}$/.test(nonce)) {
      throw new ProviderError('INVALID_CONFIG');
    }
    const settings = this.#settings.get();
    const providerId = settings.smartProcessing.selectedProviderId;
    const config = this.#configs.get(providerId);
    const operation = revisionBoundOperation(this.#configs, revision, signal);
    try {
      const front = await this.#helper.getFrontApp();
      operation.assertActive();
      const screenshot = await this.#screenshots.capture(front.windowBounds, operation.signal);
      operation.assertActive();
      const output = await this.#providers.cleanTranscript(
        config,
        {
          input: `Read the verification code visible in the screenshot. Return exactly the code and nothing else. Expected format: uppercase letters, digits, and hyphens.`,
          modelId: status.modelId,
          temperature: 0.2,
          maxOutputTokens: 32,
          image: screenshot.image,
        },
        operation.signal,
      );
      operation.assertActive();
      if (normalizeSmartOutput(output).trim() !== nonce)
        throw new ProviderError('INVALID_RESPONSE');
    } catch (error: unknown) {
      throw operation.normalize(error);
    } finally {
      operation.dispose();
    }
    if (revision !== this.#configs.smartRevision()) throw new ProviderError('STALE_CONFIG');
    const binding = this.#visionBinding(config, settings, providerId);
    this.#scavengePendingVision();
    if (this.#pendingVision.size >= 16) throw new ProviderError('UNAVAILABLE');
    const verificationId = randomUUID();
    this.#pendingVision.set(verificationId, {
      providerId: providerId as 'generic-openai' | 'litellm',
      modelId: status.modelId,
      binding,
      revision,
      expiresAt: Date.now() + 60_000,
    });
    return Object.freeze({ verificationId });
  }

  async confirmManualVision(
    verificationId: string,
    signal: AbortSignal,
  ): Promise<ReturnType<SettingsStore['get']>> {
    const pending = this.#pendingVision.get(verificationId);
    this.#pendingVision.delete(verificationId);
    if (
      pending === undefined ||
      pending.expiresAt < Date.now() ||
      pending.revision !== this.#configs.smartRevision()
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    const operation = revisionBoundOperation(this.#configs, pending.revision, signal);
    try {
      operation.assertActive();
      const settings = this.#settings.get();
      const config = this.#configs.get(pending.providerId);
      const binding = this.#visionBinding(config, settings, pending.providerId);
      if (
        settings.smartProcessing.selectedProviderId !== pending.providerId ||
        binding !== pending.binding ||
        config.modelId !== pending.modelId
      ) {
        throw new ProviderError('STALE_CONFIG');
      }
      const retained = settings.smartProcessing.visionOverrides.filter(
        (item) => item.providerId !== pending.providerId || item.modelId !== pending.modelId,
      );
      return await this.#settings.update(
        {
          smartProcessing: {
            visionOverrides: [
              ...retained,
              {
                providerId: pending.providerId,
                binding: pending.binding,
                modelId: pending.modelId,
                verifiedAt: Date.now(),
              },
            ],
          },
        },
        operation.signal,
      );
    } catch (error: unknown) {
      throw operation.normalize(error);
    } finally {
      operation.dispose();
    }
  }

  #visionBinding(
    config: Parameters<ProviderService['credentialBinding']>[0],
    settings: ReturnType<SettingsStore['get']>,
    providerId: RunnableProviderId,
  ): string {
    return `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[providerId] ?? 0)}`;
  }

  #scavengePendingVision(): void {
    const now = Date.now();
    for (const [id, pending] of this.#pendingVision) {
      if (pending.expiresAt < now) this.#pendingVision.delete(id);
    }
  }

  beginSession(
    profile: Readonly<DictationProfile> = DEFAULT_GENERAL_PROFILE,
  ): FrozenSmartTranscriptSession {
    return beginSmartSession(
      {
        settings: this.#settings,
        configs: this.#configs,
        providers: this.#providers,
        screenshots: this.#screenshots,
        helper: this.#helper,
        screenshotsDirectory: this.#screenshotsDirectory,
        retainScreenshot: (directory, screenshot) => this.#retainScreenshot(directory, screenshot),
        visionBinding: (config, settings, providerId) =>
          this.#visionBinding(config, settings, providerId),
      },
      profile,
    );
  }
}
