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
import {
  buildSmartCleanupPrompt,
  SMART_MAX_OUTPUT_TOKENS,
  SMART_TEMPERATURE,
} from './prompt-builder';
import { normalizeSmartOutput } from './output-processing';
import {
  DEFAULT_GENERAL_PROFILE,
  type DictationProfile,
} from '../../shared/schemas/dictation-profiles';

export interface SmartProcessingResult {
  readonly text: string;
  readonly screenshotFilename: string | null;
}

export interface FrozenSmartTranscriptSession {
  readonly providerId: string;
  readonly modelId: string | null;
  prepare(signal: AbortSignal): Promise<void>;
  process(text: string, signal: AbortSignal): Promise<SmartProcessingResult>;
  commitScreenshot(): void;
  cleanup(): void;
}

export interface SmartTranscriptProcessor {
  beginSession(profile?: Readonly<DictationProfile>): FrozenSmartTranscriptSession;
}

export class SmartTranscriptionService implements SmartTranscriptProcessor {
  readonly #settings: SettingsStore;
  readonly #configs: ProviderConfigService;
  readonly #providers: ProviderService;
  readonly #screenshots: ScreenshotService;
  readonly #helper: Pick<HelperClient, 'getFrontApp'>;
  readonly #screenshotsDirectory: string;
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
    readonly screenshots: ScreenshotService;
    readonly helper: Pick<HelperClient, 'getFrontApp'>;
    readonly screenshotsDirectory: string;
  }) {
    this.#settings = options.settings;
    this.#configs = options.configs;
    this.#providers = options.providers;
    this.#screenshots = options.screenshots;
    this.#helper = options.helper;
    this.#screenshotsDirectory = options.screenshotsDirectory;
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
    const binding = `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[providerId] ?? 0)}`;
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
    if (enabled) {
      const revision = this.#configs.smartRevision();
      const settings = this.#settings.get();
      const providerId = settings.smartProcessing.selectedProviderId;
      const config = this.#configs.get(providerId);
      const modelId = config.modelId;
      if (modelId === undefined || modelId === null) throw new ProviderError('INVALID_CONFIG');
      const binding = `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[providerId] ?? 0)}`;
      const capability = resolveVisionCapability({
        providerCapability: await this.#providers.preflightCapability(
          config,
          modelId,
          AbortSignal.timeout(30_000),
        ),
        providerId,
        modelId,
        binding,
        overrides: settings.smartProcessing.visionOverrides,
      });
      if (revision !== this.#configs.smartRevision()) throw new ProviderError('STALE_CONFIG');
      if (capability !== 'supported') throw new ProviderError('INVALID_CONFIG');
    }
    return this.#settings.update({
      smartProcessing: { onScreenAwarenessEnabled: enabled },
    });
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
    const binding = `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[providerId] ?? 0)}`;
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
    assertNotAborted(signal);
    const settings = this.#settings.get();
    const config = this.#configs.get(pending.providerId);
    const binding = `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[pending.providerId] ?? 0)}`;
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
    return this.#settings.update(
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
      signal,
    );
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
    const settings = structuredClone(this.#settings.get());
    const profilePrompt = profile.smartPrompt;
    const revision = this.#configs.smartRevision();
    const providerId = settings.smartProcessing.selectedProviderId;
    const config = structuredClone(this.#configs.get(providerId));
    const modelId = config.modelId ?? null;
    const vocabulary = Object.freeze(
      settings.customVocabulary.map((entry) => Object.freeze(entry)),
    );
    const binding = `${this.#providers.credentialBinding(config)}\n${String(settings.smartProcessing.credentialEpochs[providerId] ?? 0)}`;
    const osaRequested = settings.smartProcessing.onScreenAwarenessEnabled;
    let retainAllowed = settings.privacy.historyEnabled && settings.privacy.retainSmartScreenshots;
    let retained: RetainedScreenshot | null = null;
    let preparedScreenshot: CapturedScreenshot | null = null;
    let preparationError: unknown = null;
    let prepared = false;
    let used = false;
    let disposed = false;
    let revisionInvalid = false;
    const activeOperations = new Set<AbortController>();
    const removeRevision = this.#configs.subscribeSmartRevision((nextRevision) => {
      if (nextRevision === revision) return;
      revisionInvalid = true;
      for (const controller of activeOperations) controller.abort();
    });
    const removePrivacy = this.#settings.subscribe((next) => {
      if (next.privacy.historyEnabled && next.privacy.retainSmartScreenshots) return;
      retainAllowed = false;
      retained?.cleanup();
      retained = null;
    });
    const disposeSession = (): void => {
      if (disposed) return;
      disposed = true;
      removeRevision();
      removePrivacy();
      for (const controller of activeOperations) controller.abort();
      activeOperations.clear();
    };

    return Object.freeze({
      providerId,
      modelId,
      prepare: async (signal: AbortSignal): Promise<void> => {
        if (prepared || disposed) throw new ProviderError('INVALID_CONFIG');
        prepared = true;
        if (!osaRequested) return;
        const operation = sessionOperation(
          this.#configs,
          revision,
          signal,
          activeOperations,
          () => revisionInvalid,
        );
        try {
          operation.assertActive();
          if (modelId === null) throw new ProviderError('INVALID_CONFIG');
          const liveCapability = resolveVisionCapability({
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
          if (liveCapability !== 'supported') throw new ProviderError('INVALID_CONFIG');
          const front = await this.#helper.getFrontApp();
          operation.assertActive();
          preparedScreenshot = await this.#screenshots.capture(
            front.windowBounds,
            operation.signal,
          );
          operation.assertActive();
        } catch (error: unknown) {
          const normalized = operation.normalize(error);
          if (normalized instanceof ProviderError && normalized.code === 'CANCELLED') {
            throw normalized;
          }
          preparationError = normalized;
          preparedScreenshot = null;
        } finally {
          operation.dispose();
        }
      },
      process: async (text: string, signal: AbortSignal): Promise<SmartProcessingResult> => {
        if (used || disposed) throw new ProviderError('INVALID_CONFIG');
        used = true;
        const operation = sessionOperation(
          this.#configs,
          revision,
          signal,
          activeOperations,
          () => revisionInvalid,
        );
        try {
          operation.assertActive();
          if (modelId === null) throw new ProviderError('INVALID_CONFIG');
          if (!prepared) {
            if (osaRequested) throw new ProviderError('INVALID_CONFIG');
            prepared = true;
          }
          if (preparationError !== null) {
            throw preparationError instanceof Error
              ? preparationError
              : new ProviderError('INVALID_CONFIG');
          }
          const screenshot = preparedScreenshot;
          operation.assertActive();
          const output = await this.#providers.cleanTranscript(
            config,
            {
              input: buildSmartCleanupPrompt(text, vocabulary, profilePrompt),
              modelId,
              temperature: SMART_TEMPERATURE,
              maxOutputTokens: Math.min(
                config.maxOutputTokens ?? SMART_MAX_OUTPUT_TOKENS,
                SMART_MAX_OUTPUT_TOKENS,
              ),
              ...(screenshot === null ? {} : { image: screenshot.image }),
            },
            operation.signal,
          );
          operation.assertActive();
          const normalized = normalizeSmartOutput(output);
          operation.assertActive();
          const currentPrivacy = this.#settings.get().privacy;
          if (
            screenshot !== null &&
            retainAllowed &&
            currentPrivacy.historyEnabled &&
            currentPrivacy.retainSmartScreenshots
          ) {
            retained = new RetainedScreenshot(this.#screenshotsDirectory, screenshot);
          }
          operation.assertActive();
          return Object.freeze({
            text: normalized,
            screenshotFilename: retained?.filename ?? null,
          });
        } catch (error: unknown) {
          throw operation.normalize(error);
        } finally {
          operation.dispose();
        }
      },
      commitScreenshot: () => {
        if (disposed) return;
        const privacy = this.#settings.get().privacy;
        if (retainAllowed && privacy.historyEnabled && privacy.retainSmartScreenshots) {
          retained?.commit();
        } else {
          retained?.cleanup();
          retained = null;
        }
        disposeSession();
      },
      cleanup: () => {
        retained?.cleanup();
        retained = null;
        preparedScreenshot = null;
        disposeSession();
      },
    });
  }
}

function assertNotAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}

interface RevisionBoundOperation {
  readonly signal: AbortSignal;
  assertActive(): void;
  normalize(error: unknown): unknown;
  dispose(): void;
}

function revisionBoundOperation(
  configs: ProviderConfigService,
  revision: number,
  callerSignal: AbortSignal,
): RevisionBoundOperation {
  const active = new Set<AbortController>();
  let stale = false;
  const remove = configs.subscribeSmartRevision((next) => {
    if (next === revision) return;
    stale = true;
    for (const controller of active) controller.abort();
  });
  const operation = sessionOperation(configs, revision, callerSignal, active, () => stale);
  return {
    ...operation,
    dispose: () => {
      operation.dispose();
      remove();
    },
  };
}

function sessionOperation(
  configs: ProviderConfigService,
  revision: number,
  callerSignal: AbortSignal,
  active: Set<AbortController>,
  invalid: () => boolean,
): RevisionBoundOperation {
  const controller = new AbortController();
  const abort = (): void => controller.abort();
  callerSignal.addEventListener('abort', abort, { once: true });
  active.add(controller);
  const stale = (): boolean => invalid() || configs.smartRevision() !== revision;
  return {
    signal: controller.signal,
    assertActive: () => {
      if (callerSignal.aborted) throw new ProviderError('CANCELLED');
      if (stale()) throw new ProviderError('STALE_CONFIG');
      if (controller.signal.aborted) throw new ProviderError('CANCELLED');
    },
    normalize: (error) => {
      if (callerSignal.aborted) return new ProviderError('CANCELLED');
      if (stale()) return new ProviderError('STALE_CONFIG');
      return error;
    },
    dispose: () => {
      callerSignal.removeEventListener('abort', abort);
      active.delete(controller);
    },
  };
}
