import { endpointObservability } from './installed-observation-schema';
import {
  furthestObservationBoundary,
  hasCompleteDedicatedTraversal,
} from './physical-observation-progress';
import type {
  InstalledAcceptanceHelper,
  InstalledObservationRequest,
  InstalledObservationContext,
  ObservationWriter,
} from './installed-observation-types';

export async function runPhysicalObservation(
  helper: InstalledAcceptanceHelper,
  request: InstalledObservationRequest,
  context: InstalledObservationContext,
  writePipe: ObservationWriter,
): Promise<void> {
  let traversalPassed = false;
  let teardownPassed = true;
  let observationBegan = false;
  let physicalObservationStartedAt: number | null = null;
  let physicalObservationCompletedAt: number | null = null;
  let failureStage: string | null = 'helper-ready';
  let furthestBoundary: ReturnType<typeof furthestObservationBoundary> = null;
  let counterDeltas: Record<string, number> | null = null;
  let transactionDeltas: Record<string, number> | null = null;
  let baselineOwnerInstanceId: string | null = null;
  let sampledOwnerInstanceId: string | null = null;
  let ownerInstanceStable = false;
  const activatedProfiles: string[] = [];
  let widgetVisible = false;
  const removeNotifications = helper.subscribeNotifications((notification) => {
    if (notification.method !== 'activation.event') return;
    if (
      (notification.params.phase === 'down' || notification.params.phase === 'complete') &&
      !activatedProfiles.includes(notification.params.profileId)
    ) {
      activatedProfiles.push(notification.params.profileId);
    }
  });
  try {
    if (helper.readiness.status !== 'ready')
      throw new Error('Physical observation helper not ready');
    const bindings = context.profiles.map(({ id: profileId, shortcut }) => ({
      profileId,
      shortcut:
        request.automationCase === 'lifecycle' && profileId === 'general'
          ? { ...shortcut, keys: ['X', 'G'] as ('X' | 'G')[] }
          : shortcut,
    }));
    failureStage = 'activation-disable';
    const lifecycleArm =
      request.command === 'gateway-reconnect-arm' || request.command === 'electron-crash-arm';
    await helper.configureActivation(request.automationValidation && !lifecycleArm, bindings);
    failureStage = 'observation-begin';
    const baselineObservation = request.automationValidation
      ? await helper.getRuntimeObservability()
      : await helper.beginPhysicalObservation();
    observationBegan = !request.automationValidation;
    const baseline = baselineObservation.registeredInput;
    baselineOwnerInstanceId = baselineObservation.keyboardOwner.instanceId;
    sampledOwnerInstanceId = baselineOwnerInstanceId;
    if (!request.automationValidation) {
      physicalObservationStartedAt = Date.now();
      if (request.automationArmedPipe === null) {
        throw new Error('Physical observation start channel is missing');
      }
      await writePipe(request.automationArmedPipe, {
        version: 1,
        phase: 'observation-started',
        correlation: request.launchCorrelation,
        startedAt: new Date().toISOString(),
      });
    }
    if (request.automationValidation) {
      if (
        helper.activationCaptureEnabled === lifecycleArm ||
        request.automationArmedPipe === null
      ) {
        throw new Error('Native activation state was invalid at the automation arm fence');
      }
      const endpointBefore = lifecycleArm ? await endpointObservability(helper) : null;
      await writePipe(request.automationArmedPipe, {
        version: 1,
        phase: 'armed',
        correlation: request.launchCorrelation,
        activationCaptureEnabled: helper.activationCaptureEnabled,
        bindings,
        ownerIdentity: {
          instanceId: baselineObservation.keyboardOwner.instanceId,
          authenticated: baselineObservation.keyboardOwner.authenticated,
          state: baselineObservation.keyboardOwner.state,
          leaseEpoch: baselineObservation.keyboardOwner.leaseEpoch,
        },
        transactions: baselineObservation.transactions,
        endpoint: endpointBefore,
      });
      if (lifecycleArm && endpointBefore !== null) {
        failureStage = 'lifecycle-await-successor';
        const deadline = Date.now() + 60_000;
        while (Date.now() < deadline) {
          await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 250));
          try {
            const endpointAfter = await endpointObservability(helper);
            const owner = await helper.getRuntimeObservability();
            if (
              endpointAfter.gateway.processId !== endpointBefore.gateway.processId &&
              endpointAfter.owner.processId === endpointBefore.owner.processId &&
              owner.keyboardOwner.instanceId === baselineOwnerInstanceId &&
              owner.keyboardOwner.state === 'leased_disabled' &&
              !helper.activationCaptureEnabled
            ) {
              await writePipe(request.pipeName, {
                version: 1,
                result: 'passed',
                command: request.command,
                correlation: request.launchCorrelation,
                neutralAtCrash: true,
                before: endpointBefore,
                after: endpointAfter,
                ownerInstanceId: baselineOwnerInstanceId,
              });
              return;
            }
          } catch {
            // The helper transport is expected to be unavailable briefly while
            // the fixed successor reconnects to the same detached owner.
          }
        }
        throw new Error('Lifecycle successor did not reconnect within its bound');
      }
    }
    failureStage = 'physical-traversal';
    const deadline = request.automationValidation
      ? Date.now() + 120_000
      : (physicalObservationStartedAt ?? Date.now()) + 60_000;
    while (Date.now() < deadline) {
      const current = (await helper.samplePhysicalObservation()).registeredInput;
      counterDeltas = Object.fromEntries(
        Object.entries(current).map(([key, value]) => [
          key,
          value - baseline[key as keyof typeof baseline],
        ]),
      );
      furthestBoundary = furthestObservationBoundary(baseline, current);
      const sampled = await helper.getRuntimeObservability();
      sampledOwnerInstanceId = sampled.keyboardOwner.instanceId;
      ownerInstanceStable =
        baselineOwnerInstanceId.length > 0 &&
        sampledOwnerInstanceId === baselineOwnerInstanceId &&
        sampled.keyboardOwner.authenticated &&
        sampled.keyboardOwner.state === 'leased_enabled';
      transactionDeltas = Object.fromEntries(
        Object.entries(sampled.transactions)
          .filter(([, value]) => typeof value === 'number')
          .map(([key, value]) => [
            key,
            (value as number) -
              (baselineObservation.transactions[
                key as keyof typeof baselineObservation.transactions
              ] as number),
          ]),
      );
      const expectedNotifications =
        request.automationCase === 'prompt' ? 2 : request.automationCase === 'general' ? 1 : 0;
      const expectedPhysicalCallbacks = request.automationCase === 'general' ? 6 : 8;
      const automatedComplete =
        request.automationValidation &&
        ownerInstanceStable &&
        ((request.automationCase === 'general' && activatedProfiles.includes('general')) ||
          (request.automationCase === 'prompt' && activatedProfiles.includes('prompt')) ||
          request.automationCase === 'replay') &&
        (counterDeltas.physicalCallbacks ?? 0) === expectedPhysicalCallbacks &&
        (counterDeltas.registeredCandidateCallbacks ?? 0) === 1 &&
        (counterDeltas.callbackChannelAccepted ?? 0) === expectedNotifications &&
        (counterDeltas.adapterDequeued ?? 0) === expectedNotifications &&
        (counterDeltas.gatewayReceived ?? 0) === expectedNotifications &&
        (counterDeltas.v10NotificationAccepted ?? 0) === expectedNotifications &&
        (counterDeltas.electronReceived ?? 0) === expectedNotifications &&
        (transactionDeltas.committed ?? 0) === (request.automationCase === 'replay' ? 0 : 1) &&
        (transactionDeltas.replayed ?? 0) === (request.automationCase === 'replay' ? 1 : 0);
      if (
        automatedComplete ||
        (!request.automationValidation && hasCompleteDedicatedTraversal(baseline, current))
      ) {
        if (request.automationValidation) {
          failureStage = 'application-widget-path';
          widgetVisible = await context.showValidationWidget();
        }
        if (!traversalPassed) helper.recordObservationAccepted();
        traversalPassed = true;
        if (request.automationValidation) break;
      }
      await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 100));
    }
    if (!request.automationValidation) physicalObservationCompletedAt = Date.now();
  } catch {
    traversalPassed = false;
  } finally {
    if (observationBegan) {
      try {
        await within(helper.endPhysicalObservation(), 5_000, 'physical-observation-end-timeout');
      } catch {
        teardownPassed = false;
        failureStage = 'observation-end';
      }
    }
    removeNotifications();
    if (request.automationValidation) context.hideValidationWidget();
  }
  const passed =
    traversalPassed && teardownPassed && (!request.automationValidation || widgetVisible);
  if (passed) failureStage = null;
  await writePipe(request.pipeName, {
    version: 1,
    mode: request.automationValidation
      ? 'installed-automation-validation'
      : 'passive-physical-observation',
    result: passed ? 'passed' : 'failed',
    furthestBoundary,
    correlation: request.launchCorrelation,
    processId: process.pid,
    authoritative: !request.automationValidation,
    hardwareOriginObserved: !request.automationValidation && traversalPassed,
    ...(request.command === 'supplemental-synthetic-observation'
      ? { label: 'supplemental-non-authoritative' }
      : {}),
    counterDeltas,
    transactionDeltas,
    activatedProfiles,
    ownerIdentity: {
      baselineInstanceId: baselineOwnerInstanceId,
      sampledInstanceId: sampledOwnerInstanceId,
      stable: ownerInstanceStable,
    },
    widgetVisible,
    failureStage,
    observationDurationMs:
      physicalObservationStartedAt === null || physicalObservationCompletedAt === null
        ? null
        : physicalObservationCompletedAt - physicalObservationStartedAt,
  });
  if (!passed) throw new Error('Installed physical observation did not traverse every boundary');
}

async function within<T>(operation: Promise<T>, timeoutMs: number, code: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(code)), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
