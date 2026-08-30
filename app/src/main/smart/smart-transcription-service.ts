import { randomUUID } from 'node:crypto';
import type { VoiceCommand } from '../../shared/schemas/commands';
import { matchVoiceCommand } from '../commands/matcher';
import type { HelperClient } from '../helper';
import type { SettingsStore } from '../persistence/settings-store';
import type { ProviderConfigService } from '../providers/provider-config-service';
import type { ProviderService } from '../providers/provider-service';
import type { PreparedCompletionLease } from '../providers/contracts';
import { ProviderError } from '../providers/errors';
import { providerModelSelectionPolicy } from '../../shared/provider-model-selection';
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
  SMART_DEFAULT_OUTPUT_TOKENS,
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
  readonly voiceCommand?: VoiceCommand | null;
}

export interface FrozenSmartTranscriptSession {
  readonly providerId: string;
  readonly modelId: string | null;
  /** Starts provider-only work that is safe before the user submits any transcript. */
  prepareForListening?(signal: AbortSignal): Promise<void>;
  /** Prepares submit-time context and joins any provider preparation already in flight. */
  prepare(signal: AbortSignal): Promise<void>;
  process(text: string, signal: AbortSignal): Promise<SmartProcessingResult>;
  commitScreenshot(): void;
  cleanup(): void;
}

export interface SmartTranscriptProcessor {
  beginSession(profile?: Readonly<DictationProfile>): FrozenSmartTranscriptSession;
}

interface RetainedScreenshotHandle {
  readonly filename: string;
  cleanup(): void;
}

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
    const settings = structuredClone(this.#settings.get());
    const profilePrompt = profile.smartPrompt;
    const revision = this.#configs.smartRevision();
    const providerId = settings.smartProcessing.selectedProviderId;
    const config = structuredClone(this.#configs.get(providerId));
    const modelId = config.modelId ?? null;
    const modelSelectionPolicy = providerModelSelectionPolicy(providerId);
    const vocabulary = Object.freeze(
      settings.customVocabulary.map((entry) => Object.freeze(entry)),
    );
    const voiceCommands = Object.freeze(
      settings.voiceCommands.map((command) => Object.freeze(command)),
    );
    const binding = this.#visionBinding(config, settings, providerId);
    const osaRequested = settings.smartProcessing.onScreenAwarenessEnabled;
    let retainAllowed = settings.privacy.historyEnabled && settings.privacy.retainSmartScreenshots;
    let retained: RetainedScreenshotHandle | null = null;
    let preparedScreenshot: CapturedScreenshot | null = null;
    let preparationError: unknown = null;
    let completionLease: PreparedCompletionLease | null = null;
    let consumingLease: PreparedCompletionLease | null = null;
    let completionPreparation: Promise<void> | null = null;
    let preparation: Promise<void> | null = null;
    let used = false;
    let disposed = false;
    let revisionInvalid = false;
    const activeOperations = new Set<AbortController>();
    const closeCompletionLease = (reason: string): void => {
      const unused = completionLease;
      const consuming = consumingLease;
      completionLease = null;
      consumingLease = null;
      unused?.requestClose(reason);
      if (consuming !== unused) consuming?.requestClose(reason);
    };
    const removeRevision = this.#configs.subscribeSmartRevision((nextRevision) => {
      if (nextRevision === revision) return;
      revisionInvalid = true;
      closeCompletionLease('stale-config');
      for (const controller of activeOperations) controller.abort();
    });
    const removePrivacy = this.#settings.subscribe((next) => {
      if (next.privacy.historyEnabled && next.privacy.retainSmartScreenshots) return;
      retainAllowed = false;
      const revoked = retained;
      retained = null;
      try {
        revoked?.cleanup();
      } catch {
        // Privacy revocation is irreversible even if filesystem cleanup needs later scavenging.
      }
    });
    const disposeSession = (): void => {
      if (disposed) return;
      disposed = true;
      removeRevision();
      removePrivacy();
      for (const controller of activeOperations) controller.abort();
      activeOperations.clear();
      closeCompletionLease(revisionInvalid ? 'stale-config' : 'unused');
    };
    const prepareCompletion = (signal: AbortSignal): Promise<void> => {
      if (completionPreparation !== null) return completionPreparation;
      if (disposed) return Promise.reject(new ProviderError('INVALID_CONFIG'));
      if (providerId !== 'pi') {
        completionPreparation = Promise.resolve();
        return completionPreparation;
      }
      completionPreparation = (async () => {
        const operation = sessionOperation(
          this.#configs,
          revision,
          signal,
          activeOperations,
          () => revisionInvalid,
        );
        let candidate: PreparedCompletionLease | null = null;
        try {
          operation.assertActive();
          candidate = await this.#providers.prepareCompletion(config, operation.signal);
          operation.assertActive();
          completionLease = candidate;
          candidate = null;
        } catch (error: unknown) {
          candidate?.requestClose(revisionInvalid ? 'stale-config' : 'cancelled');
          const normalized = operation.normalize(error);
          if (
            normalized instanceof ProviderError &&
            (normalized.code === 'CANCELLED' || normalized.code === 'STALE_CONFIG')
          ) {
            throw normalized;
          }
          preparationError = normalized;
        } finally {
          operation.dispose();
        }
      })();
      return completionPreparation;
    };
    const prepareSession = (signal: AbortSignal): Promise<void> => {
      if (preparation !== null) return preparation;
      if (disposed) return Promise.reject(new ProviderError('INVALID_CONFIG'));
      preparation = (async () => {
        const operation = sessionOperation(
          this.#configs,
          revision,
          signal,
          activeOperations,
          () => revisionInvalid,
        );
        try {
          operation.assertActive();
          const screenshotPreparation = (async (): Promise<CapturedScreenshot | null> => {
            if (!osaRequested) return null;
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
            return await this.#screenshots.capture(front.windowBounds, operation.signal);
          })();
          const [, screenshot] = await Promise.all([
            prepareCompletion(operation.signal),
            screenshotPreparation,
          ]);
          operation.assertActive();
          preparedScreenshot = screenshot;
        } catch (error: unknown) {
          const normalized = operation.normalize(error);
          if (
            normalized instanceof ProviderError &&
            (normalized.code === 'CANCELLED' || normalized.code === 'STALE_CONFIG')
          ) {
            throw normalized;
          }
          preparationError = normalized;
          preparedScreenshot = null;
        } finally {
          operation.dispose();
        }
      })();
      return preparation;
    };

    return Object.freeze({
      providerId,
      modelId,
      prepareForListening: prepareCompletion,
      prepare: prepareSession,
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
          if (modelId === null && modelSelectionPolicy === 'required') {
            throw new ProviderError('INVALID_CONFIG');
          }
          await prepareSession(operation.signal);
          operation.assertActive();
          const fallbackFromPreparation =
            preparationError instanceof ProviderError && preparationError.fallbackEligible;
          if (preparationError !== null && !fallbackFromPreparation) {
            throw preparationError instanceof Error
              ? preparationError
              : new ProviderError('INVALID_CONFIG');
          }
          const screenshot = preparedScreenshot;
          const request = {
            input: buildSmartCleanupPrompt(text, vocabulary, profilePrompt, voiceCommands),
            ...(modelId === null ? {} : { modelId }),
            temperature: SMART_TEMPERATURE,
            maxOutputTokens: config.maxOutputTokens ?? SMART_DEFAULT_OUTPUT_TOKENS,
            ...(screenshot === null ? {} : { image: screenshot.image }),
          };
          operation.assertActive();
          const lease = completionLease;
          completionLease = null;
          let output: string;
          if (lease === null) {
            output = await this.#providers.cleanTranscript(config, request, operation.signal);
          } else {
            consumingLease = lease;
            try {
              output = await lease.complete(request, operation.signal);
            } catch (error: unknown) {
              operation.assertActive();
              if (!(error instanceof ProviderError && error.fallbackEligible)) throw error;
              output = await this.#providers.cleanTranscript(config, request, operation.signal);
            } finally {
              if (consumingLease === lease) consumingLease = null;
            }
          }
          operation.assertActive();
          const normalized = normalizeSmartOutput(output);
          operation.assertActive();
          // Smart processing may repair a translated or slightly misheard command, but execution
          // remains host-authoritative: the complete model output must equal a saved trigger.
          const smartMatch = matchVoiceCommand(normalized, voiceCommands);
          const voiceCommand = smartMatch?.kind === 'exact' ? smartMatch.command : null;
          const currentPrivacy = this.#settings.get().privacy;
          if (
            screenshot !== null &&
            retainAllowed &&
            currentPrivacy.historyEnabled &&
            currentPrivacy.retainSmartScreenshots
          ) {
            try {
              retained = this.#retainScreenshot(this.#screenshotsDirectory, screenshot);
            } catch {
              retained = null;
            }
          }
          operation.assertActive();
          return Object.freeze({
            text: normalized,
            screenshotFilename: retained?.filename ?? null,
            ...(voiceCommand === null ? {} : { voiceCommand }),
          });
        } catch (error: unknown) {
          throw operation.normalize(error);
        } finally {
          operation.dispose();
        }
      },
      commitScreenshot: () => {
        if (disposed) return;
        const current = retained;
        retained = null;
        try {
          const privacy = this.#settings.get().privacy;
          if (!(retainAllowed && privacy.historyEnabled && privacy.retainSmartScreenshots)) {
            current?.cleanup();
          }
        } finally {
          disposeSession();
        }
      },
      cleanup: () => {
        closeCompletionLease(revisionInvalid ? 'stale-config' : 'unused');
        const current = retained;
        retained = null;
        preparedScreenshot = null;
        try {
          current?.cleanup();
        } finally {
          disposeSession();
        }
      },
    });
  }
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
      if (controller.signal.aborted) return new ProviderError('CANCELLED');
      return error;
    },
    dispose: () => {
      callerSignal.removeEventListener('abort', abort);
      active.delete(controller);
    },
  };
}
