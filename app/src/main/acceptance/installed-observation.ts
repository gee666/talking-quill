import { createHash } from 'node:crypto';
import net from 'node:net';
import { isDeepStrictEqual } from 'node:util';
import { resolve } from 'node:path';
import { z, type ZodType } from 'zod';
import { HelperOwnerObservabilitySchema } from '../../shared/helper/protocol';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';
import type { HelperClient } from '../helper';
import {
  furthestObservationBoundary,
  hasCompleteDedicatedTraversal,
} from '../echo/activation-test-controller';
import type { AcceptanceRunRequestPayload } from './authorization-schema';

const EndpointPeerSchema = z
  .object({
    processId: z.number().int().positive().max(0xffff_ffff),
    creationMarker: z.string().regex(/^[1-9][0-9]{0,19}$/u),
    integrityRid: z.number().int().min(0).max(0xffff_ffff),
    sessionId: z.number().int().min(0).max(0xffff_ffff),
    userSidHash: z.string().regex(/^[0-9a-f]{64}$/u),
  })
  .strict();
export const EndpointObservabilitySchema = z
  .object({
    endpointVersion: z.literal(2),
    peerAuthenticated: z.literal(true),
    releaseBuildDigest: z.string().regex(/^[0-9a-f]{64}$/u),
    manifestSha256: z.string().regex(/^[0-9a-f]{64}$/u),
    gateway: EndpointPeerSchema,
    owner: EndpointPeerSchema,
  })
  .strict()
  .superRefine((value, context) => {
    if (value.gateway.sessionId !== value.owner.sessionId) {
      context.addIssue({
        code: 'custom',
        path: ['owner', 'sessionId'],
        message: 'Authenticated endpoint peers must share a Windows session',
      });
    }
    if (value.gateway.userSidHash !== value.owner.userSidHash) {
      context.addIssue({
        code: 'custom',
        path: ['owner', 'userSidHash'],
        message: 'Authenticated endpoint peers must share a redacted user identity',
      });
    }
  });
export const PauseLeaseRenewalSchema = z
  .object({
    pauseDurationMs: z.literal(6_500),
    beforeTimestampMs: z.number().int().nonnegative(),
    afterTimestampMs: z.number().int().nonnegative(),
    before: HelperOwnerObservabilitySchema,
    after: HelperOwnerObservabilitySchema,
  })
  .strict()
  .superRefine((value, context) => {
    if (value.afterTimestampMs - value.beforeTimestampMs < value.pauseDurationMs) {
      context.addIssue({
        code: 'custom',
        path: ['afterTimestampMs'],
        message: 'Lease-renewal pause did not span the fixed duration',
      });
    }
    if (value.after.leaseExpired !== value.before.leaseExpired + 1) {
      context.addIssue({
        code: 'custom',
        path: ['after', 'leaseExpired'],
        message: 'Lease-renewal pause must prove exactly one expiry',
      });
    }
    if (value.after.leaseRenewed !== value.before.leaseRenewed) {
      context.addIssue({
        code: 'custom',
        path: ['after', 'leaseRenewed'],
        message: 'Lease renewal changed during the pause',
      });
    }
  });

async function endpointObservability(helper: InstalledAcceptanceHelper) {
  return EndpointObservabilitySchema.parse(
    await helper.requestAcceptance(
      'acceptance.endpoint_observability',
      EndpointObservabilitySchema,
      3_000,
    ),
  );
}

async function pauseLeaseRenewal(helper: InstalledAcceptanceHelper) {
  return PauseLeaseRenewalSchema.parse(
    await helper.requestAcceptance(
      'acceptance.pause_lease_renewal',
      PauseLeaseRenewalSchema,
      10_000,
    ),
  );
}

export interface InstalledAcceptanceHelper extends HelperClient {
  requestAcceptance(method: string, resultSchema: ZodType, timeoutMs: number): Promise<unknown>;
}

export interface InstalledObservationRequest {
  readonly command: AcceptanceRunRequestPayload['command'];
  readonly heartbeatDurationMs: 6_250 | 120_000;
  readonly pipeName: string;
  readonly launchCorrelation: string;
  readonly physicalObservation: boolean;
  readonly automationValidation: boolean;
  readonly automationArmedPipe: string | null;
  readonly automationCase: string | null;
  readonly expectedUserDataRoot: string | null;
}

export interface InstalledObservationContext {
  readonly profiles: readonly DictationProfile[];
  readonly persistentWindowRolesReady: boolean;
  readonly userDataRoot: string;
  readonly showValidationWidget: () => Promise<boolean>;
  readonly hideValidationWidget: () => void;
  readonly windowsLoginStart: boolean;
  readonly mainWindowVisible: boolean;
  readonly waitForIgnoredLoginStart: (timeoutMs: number) => Promise<boolean>;
  readonly probeDiagnostics: () => Promise<{
    readonly enabled: boolean;
    readonly injectedFailureContained: boolean;
  }>;
}

export async function runInstalledObservation(
  helper: InstalledAcceptanceHelper,
  request: InstalledObservationRequest,
  context: InstalledObservationContext,
): Promise<void> {
  if (request.physicalObservation || request.automationValidation) {
    await runPhysicalObservation(helper, request, context);
    return;
  }
  await runReadinessObservation(helper, request, context);
}

async function runReadinessObservation(
  helper: InstalledAcceptanceHelper,
  request: InstalledObservationRequest,
  context: InstalledObservationContext,
): Promise<void> {
  let passed = false;
  let stage = 'helper-ready';
  let failureStage: string | null = null;
  let heartbeatEvidence: {
    durationMs: number;
    ownerInstanceId: string;
    leaseEpoch: number;
    renewalsBefore: number;
    renewalsAfter: number;
    expiriesBefore: number;
    expiriesAfter: number;
    samples: readonly {
      sampledAt: string;
      ready: boolean;
      ownerInstanceId: string;
      ownerPid: number;
      ownerCreationMarker: string;
    }[];
  } | null = null;
  try {
    if (
      request.expectedUserDataRoot !== null &&
      resolve(context.userDataRoot) !== resolve(request.expectedUserDataRoot)
    ) {
      failureStage = 'user-data-root';
      throw new Error('Installed lifecycle did not use the exact requested user-data root');
    }
    if (!context.persistentWindowRolesReady) {
      failureStage = 'application-window-roles';
      throw new Error('TalkingQuillApplication did not create every persistent window role');
    }
    if (helper.readiness.status !== 'ready') {
      failureStage = `helper-readiness-${helper.readiness.status}-${helper.readiness.reason ?? 'none'}-${helper.nativeLaunchFailure ?? 'native-unspecified'}`;
      throw new Error('Local helper did not become ready');
    }
    const bindings = context.profiles.map(({ id: profileId, shortcut }) => ({
      profileId,
      shortcut,
    }));
    const disabledReadback = { enabled: false, bindings };
    stage = 'activation-configure-disabled-first';
    const firstReadback = await helper.configureActivation(false, bindings);
    if (
      !isDeepStrictEqual(firstReadback, disabledReadback) ||
      helper.activationCaptureEnabled !== false
    ) {
      throw new Error('Disabled-first activation configuration did not round-trip exactly');
    }
    if (request.command === 'lease-expiry-arm') {
      stage = 'acceptance-lease-renewal-pause';
      const transactionBefore = (await helper.getRuntimeObservability()).transactions;
      const leaseExpiry = await pauseLeaseRenewal(helper);
      const runtimeAfter = await helper.getRuntimeObservability();
      const transactionAfter = runtimeAfter.transactions;
      const transactionDelta =
        transactionAfter.committed -
        transactionBefore.committed +
        (transactionAfter.replayed - transactionBefore.replayed);
      await writePipe(request.pipeName, {
        version: 1,
        result: 'passed',
        command: request.command,
        correlation: request.launchCorrelation,
        leaseExpiry: {
          ...leaseExpiry,
          captureStayedDisabled: !helper.activationCaptureEnabled,
          observedFinalState: runtimeAfter.keyboardOwner.state,
          transactionDelta,
        },
      });
      return;
    }
    const enabledReadback = { enabled: true, bindings };
    stage = 'activation-configure-enabled';
    const activeReadback = await helper.configureActivation(true, bindings);
    stage = 'activation-readback';
    if (!isDeepStrictEqual(activeReadback, enabledReadback)) {
      throw new Error('Enabled activation configuration did not round-trip exactly');
    }
    stage = 'runtime-observability';
    let observability = await helper.getRuntimeObservability();
    for (
      let attempt = 0;
      attempt < 20 && observability.keyboardOwner.state !== 'leased_enabled';
      attempt += 1
    ) {
      await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 100));
      observability = await helper.getRuntimeObservability();
    }
    stage = `owner-readiness-${observability.keyboardOwner.state}-${observability.keyboardOwner.authenticated ? 'authenticated' : 'unauthenticated'}-${observability.keyboardOwner.leaseEpoch === null ? 'no-lease' : 'leased'}`;
    if (
      !observability.keyboardOwner.authenticated ||
      observability.keyboardOwner.leaseEpoch === null ||
      observability.keyboardOwner.state !== 'leased_enabled'
    ) {
      throw new Error('Local keyboard owner did not report an enabled authenticated lease');
    }

    if (request.command === 'endpoint-peer') {
      stage = 'acceptance-endpoint-observability';
      const endpoint = await endpointObservability(helper);
      await writePipe(request.pipeName, {
        version: 1,
        result: 'passed',
        command: request.command,
        correlation: request.launchCorrelation,
        endpoint,
      });
      return;
    }
    if (request.command === 'login-marker') {
      if (request.automationArmedPipe === null) throw new Error('Login observation arm is missing');
      await writePipe(request.automationArmedPipe, {
        version: 1,
        phase: 'armed',
        correlation: request.launchCorrelation,
        windowsLoginStart: context.windowsLoginStart,
        mainWindowVisible: context.mainWindowVisible,
      });
      const secondLaunchIgnored = await context.waitForIgnoredLoginStart(15_000);
      await writePipe(request.pipeName, {
        version: 1,
        result: secondLaunchIgnored ? 'passed' : 'failed',
        command: request.command,
        correlation: request.launchCorrelation,
        windowsLoginStart: context.windowsLoginStart,
        mainWindowVisible: context.mainWindowVisible,
        secondLaunchIgnored,
      });
      if (!secondLaunchIgnored) throw new Error('Second login-start launch was not observed');
      return;
    }
    if (request.command === 'supplemental-synthetic-observation') {
      await writePipe(request.pipeName, {
        version: 1,
        result: 'passed',
        command: request.command,
        correlation: request.launchCorrelation,
        authoritative: false,
        label: 'supplemental-non-authoritative',
      });
      return;
    }
    if (request.command === 'diagnostics-disabled-failure') {
      const diagnostics = await context.probeDiagnostics();
      await writePipe(request.pipeName, {
        version: 1,
        result: 'passed',
        command: request.command,
        correlation: request.launchCorrelation,
        diagnostics,
      });
      return;
    }
    if (process.platform === 'win32') {
      // Keep this on the installed application path. It exercises the built
      // gateway, built owner runtime, and WindowsPrivatePipeStream across the
      // real five-second lease window instead of substituting a fake transport.
      stage = 'owner-timed-heartbeat';
      const ownerInstanceId = observability.keyboardOwner.instanceId;
      const leaseEpoch = observability.keyboardOwner.leaseEpoch;
      const initialAcceptanceEndpoint =
        request.command === 'heartbeat-120s' ? await endpointObservability(helper) : null;
      const renewalsBefore = observability.owner.leaseRenewed;
      const expiriesBefore = observability.owner.leaseExpired;
      stage = 'owner-timed-heartbeat-permissions';
      await helper.getPermissions();
      stage = 'owner-timed-heartbeat-ping';
      await helper.ping();
      stage = 'owner-timed-heartbeat-session';
      await helper.setSessionCapture('recording');
      await helper.setSessionCapture('off');
      stage = 'owner-timed-heartbeat-identity';
      const heartbeatStarted = Date.now();
      const samples: {
        sampledAt: string;
        ready: boolean;
        ownerInstanceId: string;
        ownerPid: number;
        ownerCreationMarker: string;
      }[] = [];
      const heartbeatDeadline = heartbeatStarted + request.heartbeatDurationMs;
      while (Date.now() < heartbeatDeadline) {
        await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 250));
        await helper.ping();
        observability = await helper.getRuntimeObservability();
        const sampledEndpoint =
          request.command === 'heartbeat-120s' ? await endpointObservability(helper) : null;
        samples.push({
          sampledAt: new Date().toISOString(),
          ready: observability.keyboardOwner.state === 'leased_enabled',
          ownerInstanceId: observability.keyboardOwner.instanceId,
          ownerPid: sampledEndpoint?.owner.processId ?? 0,
          ownerCreationMarker: sampledEndpoint?.owner.creationMarker ?? '',
        });
        if (
          !observability.keyboardOwner.authenticated ||
          observability.keyboardOwner.instanceId !== ownerInstanceId ||
          observability.keyboardOwner.leaseEpoch !== leaseEpoch ||
          sampledEndpoint?.owner.processId !== initialAcceptanceEndpoint?.owner.processId ||
          sampledEndpoint?.owner.creationMarker !== initialAcceptanceEndpoint?.owner.creationMarker
        ) {
          throw new Error('Owner identity or lease epoch changed during timed heartbeat');
        }
      }
      stage = `owner-timed-heartbeat-counters-${String(renewalsBefore)}-${String(observability.owner.leaseRenewed)}-${String(expiriesBefore)}-${String(observability.owner.leaseExpired)}-${observability.keyboardOwner.state}`;
      if (
        observability.owner.leaseRenewed <= renewalsBefore ||
        observability.owner.leaseExpired !== expiriesBefore ||
        observability.keyboardOwner.state !== 'leased_enabled'
      ) {
        throw new Error('Owner heartbeat did not renew cleanly across the expiry window');
      }
      heartbeatEvidence = {
        durationMs: Date.now() - heartbeatStarted,
        ownerInstanceId,
        leaseEpoch,
        renewalsBefore,
        renewalsAfter: observability.owner.leaseRenewed,
        expiriesBefore,
        expiriesAfter: observability.owner.leaseExpired,
        samples,
      };
    }
    passed = true;
  } catch {
    failureStage ??= stage;
  }
  await writePipe(request.pipeName, {
    version: 1,
    result: passed ? 'passed' : 'failed',
    checks: [
      'talking-quill-application-running',
      'main-capture-widget-created',
      'persisted-profile-activation',
      'local-owner-launch',
      'owner-authenticated',
      'disabled-first',
      'activation-configure-canonical-readback',
      'hook-installed',
      'lease-enabled',
      'configuration-revision-active',
      'windows-private-pipe-five-second-heartbeat',
      'stable-owner-identity-and-lease-epoch',
      'lease-renewed-without-expiry',
      'nonphysical-activation-protocol-ready',
      'nonphysical-observability',
    ],
    bindingCount: context.profiles.length,
    userDataRootSha256: createHash('sha256').update(resolve(context.userDataRoot)).digest('hex'),
    heartbeatEvidence,
    ...(failureStage === null ? {} : { failureStage }),
  });
  if (!passed) throw new Error('Installed readiness test failed');
}

async function runPhysicalObservation(
  helper: InstalledAcceptanceHelper,
  request: InstalledObservationRequest,
  context: InstalledObservationContext,
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

function writePipe(pipeName: string, value: unknown): Promise<void> {
  const evidence =
    value !== null && typeof value === 'object'
      ? { ...value, runtimeLifecycleAuthoritative: false }
      : value;
  return new Promise((resolveWrite, reject) => {
    const socket = net.connect(pipeName);
    socket.once('error', reject);
    socket.once('connect', () => socket.end(`${JSON.stringify(evidence)}\n`, resolveWrite));
  });
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
