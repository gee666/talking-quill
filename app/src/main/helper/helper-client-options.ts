import {
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from './helper-process';
import { type HelperRuntimeObservability } from '../../shared/helper/protocol';
import { type HelperPlatform } from './helper-path';
import { type HelperOwnerConnectionDiagnostic } from './helper-client-diagnostics';

export const ACTIVATION_CAPTURE_ROLLBACK_ENV = 'TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE' as const;

export function activationCaptureRollbackEnabled(environment: NodeJS.ProcessEnv): boolean {
  return environment[ACTIVATION_CAPTURE_ROLLBACK_ENV] === '1';
}

// The gateway launches the adjacent owner and completes their mutually
// authenticated private handshake before it accepts Electron RPC work.
export const HANDSHAKE_TIMEOUT_MS = 30_000;
export const REQUEST_TIMEOUT_MS = 3_000;
export const HEARTBEAT_INTERVAL_MS = 5_000;
// Both native adapters own one 1.5-second drain deadline. The host waits a
// comfortably larger platform envelope before best-effort termination.
export const MAC_SHUTDOWN_WAIT_MS = 3_000;
export const WINDOWS_SHUTDOWN_WAIT_MS = 3_000;
export const PREDECESSOR_DRAIN_WAIT_MS = 3_000;
export const SHUTDOWN_EXIT_MARGIN_MS = 500;
export const MAX_TERMINAL_OBSERVABILITY_LINE_BYTES = 16 * 1024;
export const MAX_DIAGNOSTIC_ACKS_IN_FLIGHT = 64;
export const OWNER_DIAGNOSTIC_JOURNAL_ENV = 'TALKING_QUILL_OWNER_DIAGNOSTIC_JOURNAL_V1';
export const STARTUP_STDERR_DRAIN_MS = 250;
export const FAILURE_WINDOW_MS = 2 * 60_000;
export const FAILURE_LIMIT = 5;
export const RESTART_DELAYS_MS = [250, 1_000, 4_000, 15_000, 30_000] as const;

export type SpawnHelper = (
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

export type HelperRuntimeObservabilitySource = 'runtime' | 'shutdown' | 'failure';

export interface HelperClientOptions {
  readonly executablePath: string;
  readonly expectedHelperVersion: string;
  readonly diagnosticJournalPath?: string;
  readonly platform: HelperPlatform;
  readonly architecture: 'x64' | 'arm64';
  /** Internal emergency gate; it disables global activation in the helper process. */
  readonly disableActivationCapture?: boolean;
  /** Test-only timing override; production uses the platform shutdown envelope. */
  readonly nativeDrainEnvelopeMs?: number;
  /** Test-only timing override for the one dispatched predecessor. */
  readonly predecessorDrainEnvelopeMs?: number;
  readonly observeRuntimeObservability?: (
    observability: HelperRuntimeObservability,
    source: HelperRuntimeObservabilitySource,
  ) => void | Promise<void>;
  readonly observeProcessLifecycle?: (event: {
    readonly phase: 'started' | 'exited';
    readonly exitCode?: number | null;
    readonly signal?: NodeJS.Signals | null;
    readonly planned?: boolean;
  }) => void | Promise<void>;
  readonly observeOwnerConnectionDiagnostic?: (
    diagnostic: HelperOwnerConnectionDiagnostic,
  ) => Promise<boolean>;
  readonly spawnHelper?: SpawnHelper;
}
