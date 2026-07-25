import { PublicAppError } from '../security/public-error';
import { HELPER_PROTOCOL_VERSION } from '../../shared/helper/protocol';
import {
  WelcomeStepSchema,
  type WelcomeState,
  type WelcomeStep,
} from '../../shared/schemas/welcome';
import type { ActivationKey } from '../../shared/helper/protocol';
import {
  GENERAL_PROFILE_ID,
  type DictationProfileId,
} from '../../shared/schemas/dictation-profiles';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type { SettingsStore } from '../persistence/settings-store';
import type { SettingsPatch } from '../../shared/schemas/settings';

const USABLE_RMS_THRESHOLD = 0.005;
const MIN_OBSERVED_SAMPLES = 1_600;

export interface WelcomePrerequisites {
  readonly microphoneReady: () => boolean;
  readonly microphoneObservation?: () => {
    readonly boundDeviceId: string | null;
    readonly observedRms: number;
    readonly sampleCount: number;
  } | null;
  readonly modelReady: () => Promise<boolean>;
  readonly modelRevision?: (modelId: WhisperModelId) => string;
  readonly helperReady: () => boolean;
  readonly helperReadinessGeneration?: () => number;
  readonly activationGestureRecognized: () => {
    readonly profileId: DictationProfileId;
    readonly activationKey: ActivationKey;
    readonly shift: boolean;
  } | null;
}

/** Owns serialized onboarding validation and persistence. */
export class WelcomeService {
  readonly #settings: SettingsStore;
  readonly #prerequisites: WelcomePrerequisites;
  readonly #now: () => number;
  #operation: Promise<unknown> = Promise.resolve();
  #invalidationGeneration = 0;
  #stepUpdateController: AbortController | null = null;
  #completionUpdateController: AbortController | null = null;

  constructor(
    settings: SettingsStore,
    prerequisites: WelcomePrerequisites,
    now: () => number = Date.now,
  ) {
    this.#settings = settings;
    this.#prerequisites = prerequisites;
    this.#now = now;
  }

  state(reopened = false): WelcomeState {
    return Object.freeze({ ...this.#settings.get().welcome, reopened });
  }

  async invalidateMicrophoneBinding(): Promise<void> {
    await this.#invalidate(2, {
      microphoneTested: false,
      microphoneEvidence: null,
    });
  }

  async invalidateModelSelection(): Promise<void> {
    await this.#invalidate(3, { modelEvidence: null });
  }

  async invalidateActivationBinding(): Promise<void> {
    await this.#invalidate(4, {
      activationTested: false,
      activationEvidence: null,
    });
  }

  setStep(step: WelcomeStep): Promise<WelcomeState> {
    return this.#serialize(async () => {
      const parsed = WelcomeStepSchema.parse(step);
      const currentWelcome = this.#settings.get().welcome;
      const current = currentWelcome.lastStep;
      if (parsed === current) return this.state();
      if (parsed > current + 1) throw prerequisiteError('Complete each setup step in order.');

      const revision = currentWelcome.revision ?? 0;
      const invalidationGeneration = this.#invalidationGeneration;
      const evidence = parsed > current ? await this.#evidenceForLeaving(current) : {};
      if (
        (this.#settings.get().welcome.revision ?? 0) !== revision ||
        this.#invalidationGeneration !== invalidationGeneration
      ) {
        throw setupChangedError();
      }

      const updateController = new AbortController();
      this.#stepUpdateController = updateController;
      try {
        try {
          await this.#settings.update(
            {
              welcome: {
                ...evidence,
                lastStep: parsed,
                revision: revision + 1,
              },
            },
            updateController.signal,
          );
        } catch (cause: unknown) {
          if (updateController.signal.aborted) throw setupChangedError();
          throw cause;
        }
        if (this.#invalidationGeneration !== invalidationGeneration) {
          throw setupChangedError();
        }
        return this.state();
      } finally {
        if (this.#stepUpdateController === updateController) {
          this.#stepUpdateController = null;
        }
      }
    });
  }

  complete(): Promise<WelcomeState> {
    const invalidationGeneration = this.#invalidationGeneration;
    return this.#serialize(async () => {
      const currentWelcome = this.#settings.get().welcome;
      if (currentWelcome.completedAt !== null) return this.state();
      if (currentWelcome.lastStep !== 6) {
        throw prerequisiteError('Complete every Welcome step before finishing setup.');
      }

      const revision = currentWelcome.revision ?? 0;
      const updateController = new AbortController();
      this.#completionUpdateController = updateController;
      try {
        await this.#assertAll();
        if (
          (this.#settings.get().welcome.revision ?? 0) !== revision ||
          this.#invalidationGeneration !== invalidationGeneration
        ) {
          throw setupChangedError();
        }
        try {
          await this.#settings.update(
            {
              welcome: {
                lastStep: 6,
                completedAt: Math.max(0, Math.floor(this.#now())),
                revision: revision + 1,
              },
            },
            updateController.signal,
          );
        } catch (cause: unknown) {
          if (updateController.signal.aborted) throw setupChangedError();
          throw cause;
        }
        if (this.#invalidationGeneration !== invalidationGeneration) {
          throw setupChangedError();
        }
        return this.state();
      } finally {
        if (this.#completionUpdateController === updateController) {
          this.#completionUpdateController = null;
        }
      }
    });
  }

  async #invalidate(
    lastValidStep: WelcomeStep,
    patch: NonNullable<SettingsPatch['welcome']>,
  ): Promise<void> {
    this.#invalidationGeneration += 1;
    const shouldInvalidate =
      this.#settings.get().welcome.completedAt === null ||
      this.#completionUpdateController !== null;
    if (shouldInvalidate) {
      this.#stepUpdateController?.abort();
      this.#completionUpdateController?.abort();
    }
    await this.#serialize(async () => {
      if (!shouldInvalidate) return;
      const current = this.#settings.get().welcome;
      await this.#settings.update({
        welcome: {
          ...patch,
          completedAt: null,
          lastStep: Math.min(current.lastStep, lastValidStep) as WelcomeStep,
          revision: (current.revision ?? 0) + 1,
        },
      });
    });
  }

  async #evidenceForLeaving(step: WelcomeStep): Promise<NonNullable<SettingsPatch['welcome']>> {
    if (step === 2) {
      const observation = this.#prerequisites.microphoneObservation?.() ?? null;
      if (
        !this.#prerequisites.microphoneReady() ||
        observation === null ||
        observation.observedRms < USABLE_RMS_THRESHOLD ||
        observation.sampleCount < MIN_OBSERVED_SAMPLES
      ) {
        throw prerequisiteError('Speak during the microphone test before continuing.');
      }
      return {
        microphoneTested: true,
        microphoneEvidence: {
          ...observation,
          usableThreshold: USABLE_RMS_THRESHOLD,
          observedAt: Math.max(0, Math.floor(this.#now())),
        },
      };
    }
    if (step === 3) {
      if (!(await this.#prerequisites.modelReady())) {
        throw prerequisiteError('Finish and verify the selected Whisper model before continuing.');
      }
      const modelId = this.#settings.get().transcription.modelId;
      const manifestRevision = this.#prerequisites.modelRevision?.(modelId);
      if (manifestRevision === undefined) {
        throw prerequisiteError('The selected model manifest could not be verified.');
      }
      return {
        modelEvidence: {
          modelId,
          manifestRevision,
          verified: true,
          verifiedAt: Math.max(0, Math.floor(this.#now())),
        },
      };
    }
    if (step === 4) {
      if (!this.#prerequisites.helperReady()) {
        throw prerequisiteError('Finish keyboard helper and permission setup before continuing.');
      }
      const testedActivation = this.#prerequisites.activationGestureRecognized();
      if (testedActivation?.profileId !== GENERAL_PROFILE_ID) {
        throw prerequisiteError(
          'Test the General profile shortcut with a Quick or Extended activation gesture before continuing.',
        );
      }
      const settings = this.#settings.get();
      const testedProfile = settings.dictationProfiles.find(
        (profile) =>
          profile.id === testedActivation.profileId &&
          profile.activationKey === testedActivation.activationKey &&
          profile.shift === testedActivation.shift,
      );
      if (testedProfile === undefined) {
        throw prerequisiteError('The tested activation profile has changed. Test it again.');
      }
      return {
        activationTested: true,
        activationEvidence: {
          profileId: testedProfile.id,
          activationKey: testedProfile.activationKey,
          shift: testedProfile.shift,
          enabled: true,
          helperProtocol: HELPER_PROTOCOL_VERSION,
          readinessGeneration: this.#prerequisites.helperReadinessGeneration?.() ?? 0,
          observedAt: Math.max(0, Math.floor(this.#now())),
        },
      };
    }
    return {};
  }

  async #assertAll(): Promise<void> {
    const settings = this.#settings.get();
    const microphone = settings.welcome.microphoneEvidence;
    if (
      !this.#prerequisites.microphoneReady() ||
      microphone == null ||
      microphone.observedRms < microphone.usableThreshold
    ) {
      throw prerequisiteError('Microphone setup is no longer ready. Return to step 2.');
    }
    if (!(await this.#prerequisites.modelReady())) {
      throw prerequisiteError('The selected Whisper model is not ready. Return to step 3.');
    }
    const model = this.#settings.get().welcome.modelEvidence;
    const selectedModel = this.#settings.get().transcription.modelId;
    if (
      model?.modelId !== selectedModel ||
      model.manifestRevision !== this.#prerequisites.modelRevision?.(selectedModel)
    ) {
      throw prerequisiteError('The selected model evidence is stale. Return to step 3.');
    }
    const current = this.#settings.get();
    const activation = current.welcome.activationEvidence;
    const readinessGeneration = this.#prerequisites.helperReadinessGeneration?.() ?? 0;
    const activatedProfile =
      activation === null || activation === undefined
        ? undefined
        : current.dictationProfiles.find(
            (profile) =>
              profile.id === activation.profileId &&
              profile.activationKey === activation.activationKey &&
              profile.shift === activation.shift,
          );
    if (
      !this.#prerequisites.helperReady() ||
      !current.app.enabled ||
      activation === null ||
      activation === undefined ||
      activatedProfile === undefined ||
      activation.helperProtocol !== HELPER_PROTOCOL_VERSION ||
      activation.readinessGeneration !== readinessGeneration
    ) {
      throw prerequisiteError('The activation shortcut is not ready. Return to step 4.');
    }
  }

  #serialize<Result>(operation: () => Promise<Result>): Promise<Result> {
    const result = this.#operation.then(operation, operation);
    this.#operation = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function setupChangedError(): PublicAppError {
  return prerequisiteError('Setup changed while it was being verified. Check the steps again.');
}

function prerequisiteError(message: string): PublicAppError {
  return new PublicAppError({ code: 'UNAVAILABLE', message });
}
