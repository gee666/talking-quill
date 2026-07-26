import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import {
  WelcomeService,
  type WelcomePrerequisites,
} from '../../app/src/main/welcome/welcome-service';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
} from '../../app/src/shared/schemas/dictation-profiles';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));
const ready = (
  overrides: Partial<{
    microphone: boolean;
    model: boolean;
    helper: boolean;
    gesture: boolean;
    rms: number;
    activationKey: 'Z' | 'Q';
    activationProfileId: 'general' | 'prompt';
    activationShift: boolean;
  }> = {},
): WelcomePrerequisites => ({
  microphoneReady: () => overrides.microphone ?? true,
  microphoneObservation: () => ({
    boundDeviceId: 'test-microphone',
    observedRms: overrides.rms ?? 0.2,
    sampleCount: 3_200,
  }),
  modelReady: () => Promise.resolve(overrides.model ?? true),
  modelRevision: () => 'a'.repeat(40),
  helperReady: () => overrides.helper ?? true,
  helperReadinessGeneration: () => 1,
  activationGestureRecognized: () =>
    (overrides.gesture ?? true)
      ? {
          profileId: overrides.activationProfileId ?? 'general',
          activationKey: overrides.activationKey ?? 'Z',
          shift: overrides.activationShift ?? false,
        }
      : null,
});

describe('WelcomeService', () => {
  it('persists guarded progress, resumes after restart, and completes without a provider', async () => {
    const root = await createTestDirectory('welcome');
    roots.push(root);
    const path = join(root, 'settings.json');
    const store = new SettingsStore(path);
    await store.initialize();
    const service = new WelcomeService(store, ready(), () => 1234);
    expect(service.state()).toMatchObject({ completedAt: null, lastStep: 1 });
    await service.setStep(2);
    await service.setStep(3);
    await service.setStep(4);
    await service.setStep(5);
    await service.setStep(6);
    await store.flush();
    const restarted = new SettingsStore(path);
    await restarted.initialize();
    const resumed = new WelcomeService(restarted, ready(), () => 1234);
    expect(resumed.state()).toMatchObject({
      completedAt: null,
      lastStep: 6,
      microphoneTested: true,
      activationTested: true,
    });
    expect(await resumed.complete()).toMatchObject({ completedAt: 1234, lastStep: 6 });
    await resumed.invalidateMicrophoneBinding();
    await resumed.invalidateModelSelection();
    await resumed.invalidateActivationBinding();
    await restarted.flush();
    const unavailableRestart = new SettingsStore(path);
    await unavailableRestart.initialize();
    const completed = new WelcomeService(
      unavailableRestart,
      ready({ microphone: false, model: false, helper: false, gesture: false }),
    );
    expect(completed.state()).toMatchObject({ completedAt: 1234, lastStep: 6 });
    expect(completed.state(true).reopened).toBe(true);
  });

  it('uses the General profile identity when profile order changes', async () => {
    const root = await createTestDirectory('welcome-profile-order');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    await store.update({
      dictationProfiles: [
        structuredClone(DEFAULT_PROMPT_PROFILE),
        { ...structuredClone(DEFAULT_GENERAL_PROFILE), activationKey: 'Q' },
      ],
    });
    const service = new WelcomeService(store, ready({ activationKey: 'Q' }), () => 1234);
    await service.setStep(2);
    await service.setStep(3);
    await service.setStep(4);
    await service.setStep(5);
    expect(store.get().welcome.activationEvidence).toMatchObject({
      profileId: 'general',
      activationKey: 'Q',
      shift: false,
    });
  });

  it('requires the General profile shortcut during Welcome activation testing', async () => {
    const root = await createTestDirectory('welcome-prompt-activation');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(
      store,
      ready({ activationProfileId: 'prompt', activationKey: 'Z', activationShift: true }),
      () => 1234,
    );
    await service.setStep(2);
    await service.setStep(3);
    await service.setStep(4);
    await expect(service.setStep(5)).rejects.toThrow('General profile shortcut');
    expect(store.get().welcome.activationEvidence).toBeNull();
    expect(store.get().welcome.lastStep).toBe(4);
  });

  it('commits each step once, skips no-op writes, and completes idempotently', async () => {
    const root = await createTestDirectory('welcome-write-count');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    let updates = 0;
    store.subscribe(() => {
      updates += 1;
    });
    const service = new WelcomeService(store, ready(), () => 1234);

    await service.setStep(2);
    await service.setStep(2);
    await service.setStep(3);
    expect(store.get().welcome.microphoneEvidence).not.toBeNull();
    await service.setStep(4);
    expect(store.get().welcome.modelEvidence).not.toBeNull();
    await service.setStep(5);
    expect(store.get().welcome.activationEvidence).not.toBeNull();
    await expect(service.complete()).rejects.toThrow('every Welcome step');
    expect(updates).toBe(4);
    await service.setStep(6);
    expect(updates).toBe(5);

    const completed = await service.complete();
    expect(updates).toBe(6);
    const resumed = new WelcomeService(
      store,
      ready({ microphone: false, model: false, helper: false, gesture: false }),
    );
    expect(await resumed.complete()).toEqual(completed);
    expect(updates).toBe(6);
  });

  it('rejects completion when readiness invalidates during asynchronous verification', async () => {
    const root = await createTestDirectory('welcome-race');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    let modelChecks = 0;
    let releaseModelCheck: () => void = () => undefined;
    const delayedModel = new Promise<boolean>((resolve) => {
      releaseModelCheck = () => resolve(true);
    });
    const prerequisites = ready();
    const service = new WelcomeService(store, {
      ...prerequisites,
      modelReady: () => {
        modelChecks += 1;
        return modelChecks === 1 ? Promise.resolve(true) : delayedModel;
      },
    });
    for (const step of [2, 3, 4, 5, 6] as const) await service.setStep(step);
    const completion = service.complete();
    await Promise.resolve();
    const invalidation = service.invalidateModelSelection();
    releaseModelCheck();
    await expect(completion).rejects.toThrow('changed while');
    await invalidation;
    expect(service.state()).toMatchObject({ completedAt: null, lastStep: 3 });
  });

  it('rolls back when invalidation arrives during the final completion write', async () => {
    let blockCompletionWrite = false;
    let completionWriteStarted!: () => void;
    let releaseCompletionWrite!: () => void;
    const writeStarted = new Promise<void>((resolve) => {
      completionWriteStarted = resolve;
    });
    const writeReleased = new Promise<void>((resolve) => {
      releaseCompletionWrite = resolve;
    });
    const store = new SettingsStore('memory://welcome-completion-race', {
      io: {
        read: () => Promise.resolve(null),
        write: async (_path, value) => {
          const completedAt = (value as { welcome?: { completedAt?: number | null } }).welcome
            ?.completedAt;
          if (blockCompletionWrite && completedAt !== null && completedAt !== undefined) {
            completionWriteStarted();
            await writeReleased;
          }
        },
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();
    const service = new WelcomeService(store, ready(), () => 1234);
    for (const step of [2, 3, 4, 5, 6] as const) await service.setStep(step);
    const observedCompletionStates: (number | null)[] = [];
    store.subscribe((settings) => {
      observedCompletionStates.push(settings.welcome.completedAt);
    });

    blockCompletionWrite = true;
    const completion = service.complete();
    await writeStarted;
    const invalidation = service.invalidateModelSelection();
    releaseCompletionWrite();

    await expect(completion).rejects.toThrow('changed while');
    await invalidation;
    expect(service.state()).toMatchObject({ completedAt: null, lastStep: 3 });
    expect(observedCompletionStates).not.toContain(1234);
  });

  it('rejects a forward step when readiness invalidates during verification', async () => {
    let releaseModelCheck!: () => void;
    const delayedModel = new Promise<boolean>((resolve) => {
      releaseModelCheck = () => resolve(true);
    });
    const root = await createTestDirectory('welcome-step-race');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(store, {
      ...ready(),
      modelReady: () => delayedModel,
    });
    await service.setStep(2);
    await service.setStep(3);

    const forward = service.setStep(4);
    await Promise.resolve();
    const invalidation = service.invalidateModelSelection();
    releaseModelCheck();

    await expect(forward).rejects.toThrow('changed while');
    await invalidation;
    expect(service.state()).toMatchObject({ completedAt: null, lastStep: 3 });
  });

  it('rejects skipped steps and every unavailable Raw prerequisite', async () => {
    const root = await createTestDirectory('welcome-guards');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(store, ready({ microphone: false }));
    await expect(service.setStep(3)).rejects.toThrow('order');
    await service.setStep(2);
    await expect(service.setStep(3)).rejects.toThrow('microphone');
    expect(store.get().welcome.lastStep).toBe(2);
  });

  it('does not accept an active microphone stream with zero RMS', async () => {
    const root = await createTestDirectory('welcome-silent');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(store, ready({ rms: 0 }));
    await service.setStep(2);
    await expect(service.setStep(3)).rejects.toThrow('Speak');
    expect(store.get().welcome.microphoneEvidence).toBeNull();
  });

  it('revalidates persisted microphone evidence against the current policy at completion', async () => {
    const root = await createTestDirectory('welcome-evidence-policy');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(store, ready());
    for (const step of [2, 3, 4, 5, 6] as const) await service.setStep(step);
    const evidence = store.get().welcome.microphoneEvidence;
    if (evidence == null) throw new Error('Expected microphone evidence');
    await store.update({
      welcome: {
        microphoneEvidence: {
          ...evidence,
          observedRms: 0.001,
          usableThreshold: 0.000_1,
        },
      },
    });

    await expect(service.complete()).rejects.toThrow('Microphone setup');
    expect(service.state().completedAt).toBeNull();
  });

  it('rejects invalid step values before writing', async () => {
    const root = await createTestDirectory('welcome-invalid');
    roots.push(root);
    const store = new SettingsStore(join(root, 'settings.json'));
    await store.initialize();
    const service = new WelcomeService(store, ready());
    await expect(service.setStep(7 as never)).rejects.toThrow();
    expect(store.get().welcome.lastStep).toBe(1);
  });
});
