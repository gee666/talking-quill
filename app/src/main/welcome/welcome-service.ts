import { PublicAppError } from '../security/public-error';
import {
  WelcomeStepSchema,
  type WelcomeState,
  type WelcomeStep,
} from '../../shared/schemas/welcome';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type { SettingsStore } from '../persistence/settings-store';
import type { SettingsPatch } from '../../shared/schemas/settings';

const USABLE_RMS_THRESHOLD = 0.005;
const MIN_OBSERVED_SAMPLES = 1_600;
const MICROPHONE_STEP = 2 satisfies WelcomeStep;
const MODEL_STEP = 3 satisfies WelcomeStep;
const FINAL_STEP = 5 satisfies WelcomeStep;

export interface WelcomePrerequisites {
  readonly microphoneReady: () => boolean;
  readonly microphoneObservation: () => {
    readonly boundDeviceId: string | null;
    readonly observedRms: number;
    readonly sampleCount: number;
  } | null;
  readonly modelReady: () => Promise<boolean>;
  readonly modelRevision: (modelId: WhisperModelId) => string;
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
    await this.#invalidate(MICROPHONE_STEP, {
      microphoneTested: false,
      microphoneEvidence: null,
    });
  }

  async invalidateModelSelection(): Promise<void> {
    await this.#invalidate(MODEL_STEP, { modelEvidence: null });
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
      if (currentWelcome.lastStep !== FINAL_STEP) {
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
                lastStep: FINAL_STEP,
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
    if (step === MICROPHONE_STEP) {
      const observation = this.#prerequisites.microphoneObservation();
      if (!this.#prerequisites.microphoneReady() || !isUsableMicrophoneEvidence(observation)) {
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
    if (step === MODEL_STEP) {
      if (!(await this.#prerequisites.modelReady())) {
        throw prerequisiteError('Finish and verify the selected Whisper model before continuing.');
      }
      const modelId = this.#settings.get().transcription.modelId;
      const manifestRevision = this.#prerequisites.modelRevision(modelId);
      return {
        modelEvidence: {
          modelId,
          manifestRevision,
          verified: true,
          verifiedAt: Math.max(0, Math.floor(this.#now())),
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
      microphone.observedRms < USABLE_RMS_THRESHOLD ||
      microphone.sampleCount < MIN_OBSERVED_SAMPLES
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
      model.manifestRevision !== this.#prerequisites.modelRevision(selectedModel)
    ) {
      throw prerequisiteError('The selected model evidence is stale. Return to step 3.');
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

function isUsableMicrophoneEvidence(
  observation: ReturnType<WelcomePrerequisites['microphoneObservation']>,
): observation is NonNullable<ReturnType<WelcomePrerequisites['microphoneObservation']>> {
  return (
    observation !== null &&
    observation.observedRms >= USABLE_RMS_THRESHOLD &&
    observation.sampleCount >= MIN_OBSERVED_SAMPLES
  );
}

function setupChangedError(): PublicAppError {
  return prerequisiteError('Setup changed while it was being verified. Check the steps again.');
}

function prerequisiteError(message: string): PublicAppError {
  return new PublicAppError({ code: 'UNAVAILABLE', message });
}
