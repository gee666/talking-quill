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
import type { RunnableProviderId } from '../../shared/schemas/providers';
import {
  DEFAULT_GENERAL_PROFILE,
  type DictationProfile,
} from '../../shared/schemas/dictation-profiles';
import {
  buildSmartCleanupPrompt,
  SMART_DEFAULT_OUTPUT_TOKENS,
  SMART_TEMPERATURE,
} from './prompt-builder';
import { normalizeSmartOutput } from './output-processing';
import { sessionOperation } from './smart-operation';
import type {
  FrozenSmartTranscriptSession,
  SmartProcessingResult,
  RetainedScreenshotHandle,
} from './smart-session-types';

interface SmartSessionDependencies {
  readonly settings: SettingsStore;
  readonly configs: ProviderConfigService;
  readonly providers: ProviderService;
  readonly screenshots: Pick<ScreenshotService, 'capture' | 'permissionStatus'>;
  readonly helper: Pick<HelperClient, 'getFrontApp'>;
  readonly screenshotsDirectory: string;
  readonly retainScreenshot: (
    directory: string,
    screenshot: CapturedScreenshot,
  ) => RetainedScreenshotHandle;
  readonly visionBinding: (
    config: Parameters<ProviderService['credentialBinding']>[0],
    settings: ReturnType<SettingsStore['get']>,
    providerId: RunnableProviderId,
  ) => string;
}

export function beginSmartSession(
  dependencies: SmartSessionDependencies,
  profile: Readonly<DictationProfile> = DEFAULT_GENERAL_PROFILE,
): FrozenSmartTranscriptSession {
  const settings = structuredClone(dependencies.settings.get());
  const profilePrompt = profile.smartPrompt;
  const revision = dependencies.configs.smartRevision();
  const providerId = settings.smartProcessing.selectedProviderId;
  const config = structuredClone(dependencies.configs.get(providerId));
  const modelId = config.modelId ?? null;
  const modelSelectionPolicy = providerModelSelectionPolicy(providerId);
  const vocabulary = Object.freeze(settings.customVocabulary.map((entry) => Object.freeze(entry)));
  const voiceCommands = Object.freeze(
    settings.voiceCommands.map((command) => Object.freeze(command)),
  );
  const binding = dependencies.visionBinding(config, settings, providerId);
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
  const removeRevision = dependencies.configs.subscribeSmartRevision((nextRevision) => {
    if (nextRevision === revision) return;
    revisionInvalid = true;
    closeCompletionLease('stale-config');
    for (const controller of activeOperations) controller.abort();
  });
  const removePrivacy = dependencies.settings.subscribe((next) => {
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
        dependencies.configs,
        revision,
        signal,
        activeOperations,
        () => revisionInvalid,
      );
      let candidate: PreparedCompletionLease | null = null;
      try {
        operation.assertActive();
        candidate = await dependencies.providers.prepareCompletion(config, operation.signal);
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
        dependencies.configs,
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
            providerCapability: await dependencies.providers.preflightCapability(
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
          const front = await dependencies.helper.getFrontApp();
          operation.assertActive();
          return await dependencies.screenshots.capture(front.windowBounds, operation.signal);
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
        dependencies.configs,
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
          output = await dependencies.providers.cleanTranscript(config, request, operation.signal);
        } else {
          consumingLease = lease;
          try {
            output = await lease.complete(request, operation.signal);
          } catch (error: unknown) {
            operation.assertActive();
            if (!(error instanceof ProviderError && error.fallbackEligible)) throw error;
            output = await dependencies.providers.cleanTranscript(
              config,
              request,
              operation.signal,
            );
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
        const currentPrivacy = dependencies.settings.get().privacy;
        if (
          screenshot !== null &&
          retainAllowed &&
          currentPrivacy.historyEnabled &&
          currentPrivacy.retainSmartScreenshots
        ) {
          try {
            retained = dependencies.retainScreenshot(dependencies.screenshotsDirectory, screenshot);
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
        const privacy = dependencies.settings.get().privacy;
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
