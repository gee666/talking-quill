import {
  readinessFromOwner,
  classifyLaunchError,
  readinessReasonFromNativeLaunchFailure,
  shouldRestartAfterFailure,
} from './helper-readiness';
import { HelperClientError } from './helper-client-error';
import { type ChildProcessWithoutNullStreams } from './helper-process';
import { dirname } from 'node:path';
import { performance } from 'node:perf_hooks';
import { HELPER_PROTOCOL_VERSION, type HelperInitializeResult } from '../../shared/helper/protocol';
import {
  DEFAULT_HELPER_PERMISSIONS,
  type HelperReadinessReason,
} from '../../shared/schemas/helper-readiness';
import { type HelperRpcSession } from './helper-rpc-channel';
import { HelperBinaryError, validateHelperExecutable } from './helper-path';
import {
  ACTIVATION_CAPTURE_ROLLBACK_ENV,
  HANDSHAKE_TIMEOUT_MS,
  OWNER_DIAGNOSTIC_JOURNAL_ENV,
} from './helper-client-options';
import { type HelperClientRuntime } from './helper-client-runtime';

export async function launch(this: HelperClientRuntime): Promise<void> {
  this.clearHeartbeat();
  this.setReadiness({
    status: 'starting',
    reason: null,
    helperVersion: null,
    permissions: DEFAULT_HELPER_PERMISSIONS,
  });
  try {
    await validateHelperExecutable(this.options.executablePath, this.options.platform);
  } catch (error) {
    if (!this.desiredRunning) return;
    const reason = error instanceof HelperBinaryError ? error.reason : 'binary-invalid';
    if (this.halfOpenProbe) {
      this.recordFailure(reason);
      return;
    }
    this.setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: null,
      permissions: DEFAULT_HELPER_PERMISSIONS,
    });
    return;
  }
  if (!this.desiredRunning) return;

  this.plannedExit = null;
  this.terminating = false;
  const generation = ++this.generation;
  let child: ChildProcessWithoutNullStreams;
  try {
    child = this.spawnHelper(this.options.executablePath, {
      cwd: dirname(this.options.executablePath),
      env: Object.freeze({
        ...process.env,
        NO_COLOR: '1',
        [ACTIVATION_CAPTURE_ROLLBACK_ENV]:
          this.options.disableActivationCapture === true ? '1' : undefined,
        [OWNER_DIAGNOSTIC_JOURNAL_ENV]: this.options.diagnosticJournalPath,
      }),
      shell: false,
      detached: this.options.platform === 'win32',
      windowsHide: true,
    });
  } catch {
    this.recordFailure('spawn-failed');
    return;
  }
  this.child = child;
  const session = this.rpcChannel.attach(child);
  this.rpcSession = session;
  this.sessionAuthoritative = false;
  this.maintenancePreparing = false;
  this.maintenancePrepared = false;
  this.captureDisabledForSession = false;
  this.captureBuildDisabledForSession = false;
  this.runtimeRollbackForSession = false;
  this.sessionKeyCaptureAvailable = null;
  this.ownerAssociation = null;
  this.nativeLaunchFailure = null;
  this.activation.prepareFreshSession();
  this.attachProcess(child, session, generation);
  child.once('spawn', () => this.publishProcessLifecycle({ phase: 'started' }));

  const launchDeadline = performance.now() + HANDSHAKE_TIMEOUT_MS;
  const remainingLaunchTime = () => launchDeadline - performance.now();
  let launchOwnerReason: HelperReadinessReason | null = null;
  try {
    const initialized = await this.rpcChannel.request(
      session,
      'initialize',
      { protocolVersion: HELPER_PROTOCOL_VERSION },
      {
        timeoutMs: remainingLaunchTime(),
        timeoutReason: 'handshake-timeout',
        allowDraining: false,
        supervision: false,
      },
    );
    if (!this.isActiveChild(child, session)) return;
    this.validateHandshake(initialized);
    this.ownerAssociation = {
      instanceId: initialized.keyboardOwner.instanceId,
      buildId: initialized.keyboardOwner.buildId,
      leaseEpoch: initialized.keyboardOwner.leaseEpoch,
    };
    this.captureBuildDisabledForSession = initialized.keyboardCapture.buildDisabled;
    this.captureDisabledForSession = !initialized.keyboardCapture.activationAvailable;
    this.runtimeRollbackForSession = initialized.keyboardCapture.runtimeRollbackActive;
    this.sessionKeyCaptureAvailable = initialized.keyboardCapture.sessionKeyCaptureAvailable;
    let readiness = readinessFromOwner(
      initialized.helperVersion,
      initialized.hookStatus,
      initialized.permissions,
      initialized.keyboardOwner,
      this.captureDisabledForSession,
      initialized.keyboardCapture.runtimeRollbackActive,
    );
    launchOwnerReason = readiness.reason?.startsWith('owner-') === true ? readiness.reason : null;
    // A fresh helper is natively disabled. Keep it blocked until a second
    // health snapshot confirms that replaying retained activation is safe.
    this.activation.setBlockedByHealth(true);
    {
      const [permissions, health] = await Promise.all([
        this.rpcChannel.request(
          session,
          'permissions.get',
          {},
          {
            timeoutMs: remainingLaunchTime(),
            timeoutReason: 'handshake-timeout',
            allowDraining: false,
            supervision: true,
          },
        ),
        this.rpcChannel.request(
          session,
          'ping',
          {},
          {
            timeoutMs: remainingLaunchTime(),
            timeoutReason: 'handshake-timeout',
            allowDraining: false,
            supervision: true,
          },
        ),
      ]);
      if (!this.ownerAssociationMatches(health.keyboardOwner)) {
        throw new HelperClientError('rpc-error', 'Native helper owner association changed');
      }
      readiness = readinessFromOwner(
        initialized.helperVersion,
        health.hookStatus,
        permissions,
        health.keyboardOwner,
        this.captureDisabledForSession,
        initialized.keyboardCapture.runtimeRollbackActive,
      );
      launchOwnerReason = readiness.reason?.startsWith('owner-') === true ? readiness.reason : null;
    }
    {
      const capture = await this.rpcChannel.request(
        session,
        'session.set_capture',
        { mode: 'off' },
        {
          timeoutMs: remainingLaunchTime(),
          timeoutReason: 'handshake-timeout',
          allowDraining: false,
          supervision: true,
        },
      );
      if (capture.mode !== 'off') {
        throw new HelperClientError(
          'rpc-error',
          'Native helper did not confirm disabled session capture',
        );
      }
    }
    if (readiness.status === 'ready' && !this.captureDisabledForSession) {
      this.activation.setBlockedByHealth(false);
    }
    await this.activation.reconcileFreshHelper(
      session,
      remainingLaunchTime(),
      'handshake-timeout',
      () => {
        if (this.rpcSession === session && this.rpcChannel.isCurrent(session)) {
          this.sessionAuthoritative = true;
        }
      },
    );
    if (this.captureDisabledForSession) this.activation.setBlockedByHealth(true);
    if (!this.isActiveChild(child, session)) return;
    if (this.halfOpenProbe) {
      this.failureTimes = [];
      this.crashLoopOpen = false;
      this.halfOpenProbe = false;
    }
    this.setReadiness(readiness);
    this.flushAuthoritativeActivation();
    this.startHeartbeat(child, session);
  } catch (error) {
    if (this.child !== child || this.rpcSession !== session) return;
    const fallbackReason = classifyLaunchError(error, launchOwnerReason);
    await this.waitForStartupStderr(generation);
    if (!this.isActiveChild(child, session)) return;
    const reason =
      readinessReasonFromNativeLaunchFailure(this.nativeLaunchFailure) ?? fallbackReason;
    this.terminateCurrent(reason, shouldRestartAfterFailure(reason));
  }
}

export function validateHandshake(
  this: HelperClientRuntime,
  initialized: HelperInitializeResult,
): void {
  const expectedPlatform = this.options.platform === 'win32' ? 'windows' : 'macos';
  const expectedArchitecture = this.options.architecture === 'x64' ? 'x86_64' : 'aarch64';
  if (
    initialized.helperVersion !== this.options.expectedHelperVersion ||
    initialized.platform !== expectedPlatform ||
    initialized.architecture !== expectedArchitecture
  ) {
    throw new HelperClientError('rpc-error', 'Native helper protocol or build is incompatible');
  }
}

export function canLaunch(this: HelperClientRuntime): boolean {
  return this.desiredRunning && this.child === null;
}

export function isActiveChild(
  this: HelperClientRuntime,
  child: ChildProcessWithoutNullStreams,
  session: HelperRpcSession,
): boolean {
  return this.desiredRunning && this.child === child && this.rpcSession === session;
}
