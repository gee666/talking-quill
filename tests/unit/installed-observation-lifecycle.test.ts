import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  runInstalledObservation,
  type InstalledAcceptanceHelper,
  type InstalledObservationContext,
  type InstalledObservationRequest,
} from '../../app/src/main/acceptance/installed-observation';
import { DEFAULT_GENERAL_PROFILE } from '../../app/src/shared/schemas/dictation-profiles';

const transport = vi.hoisted(() => ({
  writes: [] as { pipe: string; value: Record<string, unknown> }[],
}));
vi.mock('node:net', () => ({
  default: {
    connect: (pipe: string) => {
      const socket = {
        once: (event: string, listener: () => void) => {
          if (event === 'connect') void Promise.resolve().then(listener);
          return socket;
        },
        end: (line: string, complete: () => void) => {
          transport.writes.push({ pipe, value: JSON.parse(line) as Record<string, unknown> });
          complete();
        },
      };
      return socket;
    },
  },
}));

afterEach(() => {
  transport.writes.length = 0;
  vi.useRealTimers();
});

function setup(overrides: Partial<InstalledObservationRequest> = {}) {
  const request: InstalledObservationRequest = {
    command: 'diagnostics-disabled-failure',
    heartbeatDurationMs: 6_250,
    pipeName: 'result',
    launchCorrelation: 'correlation',
    physicalObservation: false,
    automationValidation: false,
    automationArmedPipe: null,
    automationCase: null,
    expectedUserDataRoot: null,
    ...overrides,
  };
  const context: InstalledObservationContext = {
    profiles: [structuredClone(DEFAULT_GENERAL_PROFILE)],
    persistentWindowRolesReady: true,
    userDataRoot: 'unused',
    showValidationWidget: vi.fn(() => Promise.resolve(true)),
    hideValidationWidget: vi.fn(),
    windowsLoginStart: false,
    mainWindowVisible: false,
    waitForIgnoredLoginStart: vi.fn(() => Promise.resolve(false)),
    probeDiagnostics: vi.fn(() =>
      Promise.resolve({ enabled: false, injectedFailureContained: true }),
    ),
  };
  const counters = {
    hookInstalled: 0,
    pumpAlive: 0,
    hcActionCallbacks: 0,
    physicalCallbacks: 0,
    physicalCallbacksFiltered: 0,
    registeredCandidateCallbacks: 0,
    registeredMatchCallbacks: 0,
    registeredReleaseCallbacks: 0,
    callbackChannelAccepted: 0,
    callbackChannelRejected: 0,
    adapterDequeued: 0,
    ownerAdmitted: 0,
    ownerFlushed: 0,
    ownerRejected: 0,
    gatewayReceived: 0,
    v10NotificationAccepted: 0,
    electronReceived: 0,
    observationAccepted: 0,
  };
  const baseline = {
    registeredInput: counters,
    keyboardOwner: {
      instanceId: 'owner-1',
      authenticated: true,
      leaseEpoch: 1,
      state: 'leased_enabled',
    },
    transactions: { committed: 0, replayed: 0 },
  };
  let enabled = false;
  const removeNotifications = vi.fn();
  const helper = {
    readiness: { status: 'ready', reason: null },
    get activationCaptureEnabled() {
      return enabled;
    },
    configureActivation: vi.fn((next: boolean, bindings: unknown) => {
      enabled = next;
      return Promise.resolve({ enabled: next, bindings });
    }),
    getRuntimeObservability: vi.fn(() => Promise.resolve(structuredClone(baseline))),
    beginPhysicalObservation: vi.fn(() => Promise.resolve(structuredClone(baseline))),
    samplePhysicalObservation: vi.fn(() => Promise.resolve(structuredClone(baseline))),
    endPhysicalObservation: vi.fn(() => Promise.resolve()),
    subscribeNotifications: vi.fn<
      (
        listener: (notification: {
          method: string;
          params: { phase: string; profileId: string };
        }) => void,
      ) => () => void
    >(() => removeNotifications),
    recordObservationAccepted: vi.fn(),
  };
  return {
    request,
    context,
    helper,
    baseline,
    removeNotifications,
    run: () =>
      runInstalledObservation(helper as unknown as InstalledAcceptanceHelper, request, context),
  };
}

describe('installed observation lifecycle', () => {
  it('configures disabled-first before diagnostics and keeps pipe evidence non-authoritative', async () => {
    const value = setup();
    await value.run();
    expect(value.helper.configureActivation.mock.calls.map(([enabled]) => enabled)).toEqual([
      false,
      true,
    ]);
    expect(value.context.probeDiagnostics).toHaveBeenCalledOnce();
    expect(transport.writes).toEqual([
      {
        pipe: 'result',
        value: {
          version: 1,
          result: 'passed',
          command: 'diagnostics-disabled-failure',
          correlation: 'correlation',
          diagnostics: { enabled: false, injectedFailureContained: true },
          runtimeLifecycleAuthoritative: false,
        },
      },
    ]);
  });

  it('rejects a mismatched user-data root before touching activation', async () => {
    const value = setup({ expectedUserDataRoot: 'different-root' });
    await expect(value.run()).rejects.toThrow('Installed readiness test failed');
    expect(value.helper.configureActivation).not.toHaveBeenCalled();
    expect(transport.writes[0]?.value).toMatchObject({
      result: 'failed',
      failureStage: 'user-data-root',
      runtimeLifecycleAuthoritative: false,
    });
    expect(transport.writes[0]?.value.userDataRootSha256).toMatch(/^[0-9a-f]{64}$/u);
  });

  it('preserves login arm, command failure, and final readiness failure write order', async () => {
    const value = setup({ command: 'login-marker', automationArmedPipe: 'armed' });
    await expect(value.run()).rejects.toThrow('Installed readiness test failed');
    expect(value.context.waitForIgnoredLoginStart).toHaveBeenCalledExactlyOnceWith(15_000);
    expect(transport.writes.map(({ pipe }) => pipe)).toEqual(['armed', 'result', 'result']);
    expect(transport.writes[0]?.value).toMatchObject({ phase: 'armed' });
    expect(transport.writes[1]?.value).toMatchObject({
      command: 'login-marker',
      result: 'failed',
      secondLaunchIgnored: false,
    });
    expect(transport.writes[2]?.value).toMatchObject({
      result: 'failed',
      failureStage: 'owner-readiness-leased_enabled-authenticated-leased',
    });
  });

  it('keeps automated traversal, widget display, and notification teardown in order', async () => {
    const value = setup({
      command: 'supplemental-synthetic-observation',
      automationValidation: true,
      automationArmedPipe: 'armed',
      automationCase: 'general',
    });
    const current = structuredClone(value.baseline);
    Object.assign(current.registeredInput, {
      physicalCallbacks: 6,
      registeredCandidateCallbacks: 1,
      callbackChannelAccepted: 1,
      adapterDequeued: 1,
      gatewayReceived: 1,
      v10NotificationAccepted: 1,
      electronReceived: 1,
    });
    current.transactions.committed = 1;
    value.helper.getRuntimeObservability
      .mockResolvedValueOnce(value.baseline)
      .mockResolvedValue(current);
    value.helper.samplePhysicalObservation.mockImplementation(() => {
      value.helper.subscribeNotifications.mock.calls[0]?.[0]({
        method: 'activation.event',
        params: { phase: 'down', profileId: 'general' },
      });
      return Promise.resolve(current);
    });
    await value.run();
    expect(value.helper.recordObservationAccepted).toHaveBeenCalledOnce();
    expect(value.context.showValidationWidget).toHaveBeenCalledOnce();
    expect(value.context.hideValidationWidget).toHaveBeenCalledOnce();
    expect(value.removeNotifications).toHaveBeenCalledOnce();
    expect(value.helper.endPhysicalObservation).not.toHaveBeenCalled();
    expect(transport.writes.map(({ pipe }) => pipe)).toEqual(['armed', 'result']);
    expect(transport.writes[1]?.value).toMatchObject({
      result: 'passed',
      mode: 'installed-automation-validation',
      activatedProfiles: ['general'],
      authoritative: false,
      widgetVisible: true,
      label: 'supplemental-non-authoritative',
    });
  });

  it('bounds passive teardown and reports its failure after removing notifications', async () => {
    vi.useFakeTimers();
    const value = setup({
      command: 'manual-physical-observation',
      physicalObservation: true,
      automationArmedPipe: 'armed',
    });
    value.helper.samplePhysicalObservation.mockRejectedValue(new Error('sample failed'));
    value.helper.endPhysicalObservation.mockReturnValue(new Promise<void>(() => undefined));
    const rejected = expect(value.run()).rejects.toThrow('did not traverse every boundary');
    await vi.advanceTimersByTimeAsync(5_000);
    await rejected;
    expect(value.helper.endPhysicalObservation).toHaveBeenCalledOnce();
    expect(value.removeNotifications).toHaveBeenCalledOnce();
    expect(value.context.hideValidationWidget).not.toHaveBeenCalled();
    expect(transport.writes[0]?.value).toMatchObject({ phase: 'observation-started' });
    expect(transport.writes[1]?.value).toMatchObject({
      result: 'failed',
      failureStage: 'observation-end',
      observationDurationMs: null,
    });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('samples passive observation for the full fixed window after the first successful traversal', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const value = setup({
      command: 'manual-physical-observation',
      physicalObservation: true,
      automationArmedPipe: 'armed',
    });
    const current = structuredClone(value.baseline);
    for (const key of [
      'physicalCallbacks',
      'registeredCandidateCallbacks',
      'registeredMatchCallbacks',
      'registeredReleaseCallbacks',
      'callbackChannelAccepted',
      'adapterDequeued',
      'ownerAdmitted',
      'ownerFlushed',
      'gatewayReceived',
      'v10NotificationAccepted',
      'electronReceived',
    ] as const) {
      current.registeredInput[key] = 1;
    }
    value.helper.samplePhysicalObservation.mockResolvedValue(current);
    value.helper.getRuntimeObservability.mockResolvedValue(current);
    const operation = value.run();
    await vi.advanceTimersByTimeAsync(59_900);
    expect(value.helper.recordObservationAccepted).toHaveBeenCalledOnce();
    expect(value.helper.endPhysicalObservation).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(100);
    await operation;
    expect(value.helper.samplePhysicalObservation).toHaveBeenCalledTimes(600);
    expect(value.helper.endPhysicalObservation).toHaveBeenCalledOnce();
    expect(value.removeNotifications).toHaveBeenCalledOnce();
    expect(transport.writes[1]?.value).toMatchObject({
      result: 'passed',
      authoritative: true,
      hardwareOriginObserved: true,
      observationDurationMs: 60_000,
      runtimeLifecycleAuthoritative: false,
    });
    expect(vi.getTimerCount()).toBe(0);
  });
});
