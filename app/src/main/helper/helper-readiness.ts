import type {
  HelperInitializeResult,
  HelperKeyboardOwnerSnapshot,
  HelperPermissions,
} from '../../shared/helper/protocol';
import type { HelperReadiness, HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { HelperClientError } from './helper-client-error';

/** Recognize fixed native failure categories without forwarding arbitrary stderr. */
export function nativePasteFailureCategory(line: string): string | null {
  const categories: Readonly<Record<string, string>> = {
    'keyboard-owner paste unavailable: activation target missing': 'target-missing',
    'keyboard-owner paste unavailable: target changed': 'target-changed',
    'keyboard-owner paste unavailable: clipboard validation failed': 'clipboard-validation',
    'keyboard-owner clipboard validation: clipboard busy or absent': 'clipboard-busy',
  };
  if (Object.hasOwn(categories, line)) return categories[line] ?? null;
  const targetFailure =
    /^keyboard-owner paste target validation: (foreground-window|foreground-process|focused-control|focused-process|focus-query|focus-changed|caret-window|caret-query|caret-changed|caret-position|input-mode)$/u.exec(
      line,
    );
  const targetCategory = targetFailure?.[1];
  if (targetCategory !== undefined) return `target-${targetCategory}`;
  if (
    /^keyboard-owner clipboard validation: expired=(true|false) hash_match=(true|false) sequence_match=(true|false)$/u.test(
      line,
    )
  ) {
    return 'clipboard-content-or-sequence';
  }
  if (
    /^keyboard-owner paste unavailable: gate_or_deadline unavailable=(true|false) timed_out=(true|false)$/u.test(
      line,
    )
  ) {
    return 'gate-or-deadline';
  }
  return null;
}

export function readinessFromOwner(
  helperVersion: string | null,
  hookStatus: HelperInitializeResult['hookStatus'],
  permissions: HelperPermissions,
  owner: HelperKeyboardOwnerSnapshot,
  captureDisabled: boolean,
  runtimeRollbackActive: boolean,
): HelperReadiness {
  const unavailable = (reason: HelperReadinessReason, incompatible = false): HelperReadiness => ({
    status: incompatible ? 'incompatible' : 'unavailable',
    reason,
    helperVersion,
    permissions,
  });
  if (runtimeRollbackActive) return unavailable('owner-rollback');
  if (!owner.authenticated || owner.leaseEpoch === null) {
    return unavailable(owner.state === 'unavailable' ? 'owner-missing' : 'owner-auth-failed');
  }
  if (owner.state === 'draining') return unavailable('owner-draining');
  if (owner.state === 'maintenance') return unavailable('owner-maintenance');
  if (owner.state === 'degraded') return unavailable('owner-degraded');
  if (owner.state === 'unavailable') return unavailable('owner-missing');
  if (owner.state === 'idle') return unavailable('owner-busy');
  if (owner.state === 'safe_disabled' || captureDisabled) return unavailable('capture-disabled');

  return readinessFromHandshake(helperVersion, hookStatus, permissions);
}

function readinessFromHandshake(
  helperVersion: string | null,
  hookStatus: HelperInitializeResult['hookStatus'],
  permissions: HelperPermissions,
): HelperReadiness {
  if (permissions.inputMonitoring === 'denied') {
    return {
      status: 'permission-required',
      reason: 'input-monitoring-required',
      helperVersion,
      permissions,
    };
  }
  if (permissions.accessibility === 'denied') {
    return {
      status: 'permission-required',
      reason: 'accessibility-required',
      helperVersion,
      permissions,
    };
  }
  if (permissions.eventPost === 'denied') {
    return {
      status: 'permission-required',
      reason: 'event-post-required',
      helperVersion,
      permissions,
    };
  }
  if (!hookTransportReady(hookStatus)) {
    return { status: 'unavailable', reason: 'hook-fault', helperVersion, permissions };
  }
  // `ready` means the authenticated transport, owner protocol, hook
  // installation, and message pump are available. Physical callback delivery
  // is reported independently by hookStatus/registered-input observability.
  return { status: 'ready', reason: null, helperVersion, permissions };
}

export function hookTransportReady(hookStatus: HelperInitializeResult['hookStatus']): boolean {
  return hookStatus === 'installed_unobserved' || hookStatus === 'physical_observed';
}

export function permissionsAreGranted(permissions: HelperPermissions): boolean {
  return Object.values(permissions).every(
    (permission) => permission === 'granted' || permission === 'not_applicable',
  );
}

export function classifyLaunchError(
  error: unknown,
  ownerFallback: HelperReadinessReason | null = null,
): HelperReadinessReason {
  if (error instanceof HelperClientError) {
    if (error.code === 'request-timeout') return 'handshake-timeout';
    if (error.code === 'rpc-error') {
      const ownerReason = ownerReasonFromRpcCode(error.rpcCode);
      if (ownerReason !== null) return ownerReason;
      if (error.rpcCode === -32_001) return 'protocol-mismatch';
      if (error.message.includes('owner association changed')) return 'owner-degraded';
      if (error.message.includes('incompatible')) return ownerFallback ?? 'protocol-mismatch';
      return ownerFallback ?? 'hook-fault';
    }
  }
  return ownerFallback ?? 'malformed-response';
}

export function classifyNativeLaunchFailure(line: string): string | null {
  const hookInstall =
    /^keyboard-owner hook install unavailable: (module|access_denied|module_unavailable|native_unavailable)$/u.exec(
      line,
    );
  if (hookInstall?.[1] !== undefined) return `hook-install-${hookInstall[1]}`;
  const safeConnectFailures = new Map([
    ['talking-quill-helper: keyboard owner endpoint is unavailable', 'owner-unavailable'],
    ['talking-quill-helper: keyboard owner authentication failed', 'owner-authentication-failed'],
    ['talking-quill-helper: keyboard owner is incompatible', 'owner-incompatible'],
    ['talking-quill-helper: keyboard owner is busy or draining', 'owner-busy'],
  ]);
  return safeConnectFailures.get(line) ?? null;
}

export function readinessReasonFromNativeLaunchFailure(
  failure: string | null,
): HelperReadinessReason | null {
  if (failure === null) return null;
  if (failure === 'owner-singleton-collision') return 'owner-singleton-collision';
  if (failure === 'owner-incompatible') return 'owner-incompatible';
  if (failure === 'owner-authentication-failed') return 'owner-auth-failed';
  if (failure === 'owner-busy') return 'owner-busy';
  if (failure === 'owner-unavailable') {
    return 'owner-missing';
  }
  if (failure.startsWith('hook-install-')) return 'hook-fault';
  return null;
}

export function safeChildExitDiagnostic(
  code: number | null,
  signal: NodeJS.Signals | null,
): string {
  if (code !== null && Number.isSafeInteger(code)) return `helper-exit-code-${String(code)}`;
  if (signal !== null && /^SIG[A-Z0-9]+$/u.test(signal)) {
    return `helper-exit-signal-${signal.toLowerCase()}`;
  }
  return 'helper-exit-unknown';
}

export function isOwnerTransition(reason: HelperReadinessReason): boolean {
  return reason === 'owner-busy' || reason === 'owner-draining';
}

export function shouldRestartAfterFailure(reason: HelperReadinessReason): boolean {
  // These faults identify incompatible local binaries or an unsafe protocol stream.
  // Relaunching the same binaries cannot repair them and causes visible process churn.
  // All remaining reasons retain bounded transient supervision.
  return ![
    'protocol-mismatch',
    'malformed-response',
    'owner-incompatible',
    'owner-auth-failed',
    'owner-security-fault',
    'owner-rollback',
    'owner-indeterminate',
    'owner-singleton-collision',
  ].includes(reason);
}

export function ownerReasonFromRpcCode(rpcCode: number | null): HelperReadinessReason | null {
  switch (rpcCode) {
    case -32_005:
      return 'owner-auth-failed';
    case -32_006:
      return 'owner-incompatible';
    case -32_007:
      return 'owner-busy';
    case -32_008:
      return 'owner-draining';
    case -32_009:
      return 'owner-rollback';
    case -32_010:
      return 'owner-security-fault';
    case -32_011:
      return 'owner-indeterminate';
    case -32_012:
      return 'owner-singleton-collision';
    default:
      return null;
  }
}
