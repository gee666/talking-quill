import { waitForCloseWithin } from './helper-process';
import {
  classifyNativeLaunchFailure,
  nativePasteFailureCategory,
  safeChildExitDiagnostic,
} from './helper-readiness';
import { type ChildProcessWithoutNullStreams } from './helper-process';
import { StringDecoder } from 'node:string_decoder';
import { z } from 'zod';
import {
  HelperDiagnosticIdentitySchema,
  HelperTerminalObservabilityRecordSchema,
  type HelperRuntimeObservability,
  type HelperTerminalObservabilityRecord,
} from '../../shared/helper/protocol';
import { type HelperRpcSession } from './helper-rpc-channel';
import {
  REQUEST_TIMEOUT_MS,
  MAX_TERMINAL_OBSERVABILITY_LINE_BYTES,
  MAX_DIAGNOSTIC_ACKS_IN_FLIGHT,
  STARTUP_STDERR_DRAIN_MS,
  type HelperRuntimeObservabilitySource,
} from './helper-client-options';
import { type HelperClientRuntime } from './helper-client-runtime';

const HelperOwnerConnectionDiagnosticSchema = z
  .object({
    event: z.literal('helper.owner.connection.replay'),
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
    streamId: HelperDiagnosticIdentitySchema,
    processGeneration: z.string().regex(/^[1-9][0-9]{0,19}$/u),
    category: z.enum([
      'connect',
      'disconnected',
      'uncertain',
      'protocol',
      'acquire_rejected',
      'rejected',
      'sequence_exhausted',
      'transport',
    ]),
    operation: z.enum([
      'capture.replace_configuration',
      'capture.set_enabled',
      'command.sequence',
      'connect.reconcile',
      'established_operation',
      'front_app.get',
      'front_app.metadata.get',
      'front_app.metadata_get',
      'health.get',
      'lease.acquire',
      'lease.release',
      'lease.renew',
      'observability.get',
      'paste.await_commit',
      'paste.inject',
      'permissions.get',
      'runtime.rollback',
      'service',
      'service.poll',
      'session.reconcile_off',
      'session.set_mode',
    ]),
    correlationStatus: z.enum([
      'none',
      'not_established',
      'pending',
      'matched',
      'matched_initial_response',
      'mismatched',
      'unexpected_response',
      'unknown',
    ]),
    healthRefresh: z.enum(['not_attempted', 'succeeded', 'failed']),
    transportStatus: z.enum(['open', 'eof', 'closed', 'error', 'backpressured', 'unknown']),
    ownerProcessState: z.enum(['running', 'exited', 'unknown']),
    count: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    counterOverflow: z.boolean(),
    durable: z.boolean(),
    durabilityFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    writerStartFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    synchronizationRecoveries: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
  })
  .strict();
export type HelperOwnerConnectionDiagnostic = z.infer<typeof HelperOwnerConnectionDiagnosticSchema>;

export function attachProcess(
  this: HelperClientRuntime,
  child: ChildProcessWithoutNullStreams,
  session: HelperRpcSession,
  generation: number,
): void {
  // Stderr is never copied into diagnostics. Only the helper's strict,
  // aggregate-only terminal record can cross into the privacy-safe sink.
  const stderrDecoder = new StringDecoder('utf8');
  let stderrLine = '';
  let discardOversizedLine = false;
  let terminalCandidate: HelperTerminalObservabilityRecord | null = null;
  let duplicateTerminalCandidate = false;
  let resolveStderrDrain: () => void = () => undefined;
  const stderrDrain = {
    generation,
    promise: new Promise<void>((resolve) => {
      resolveStderrDrain = resolve;
    }),
    complete: () => {
      if (stderrDrain.completed) return;
      stderrDrain.completed = true;
      resolveStderrDrain();
    },
    completed: false,
  };
  this.stderrDrain = stderrDrain;
  const acceptStderrLine = (): void => {
    const line = stderrLine.endsWith('\r') ? stderrLine.slice(0, -1) : stderrLine;
    const parsed = discardOversizedLine ? null : this.parseDiagnosticLine(line);
    if (parsed?.kind === 'terminal') {
      if (terminalCandidate === null) terminalCandidate = parsed.value;
      else duplicateTerminalCandidate = true;
    } else if (parsed?.kind === 'owner-connection') {
      this.commitAndAcknowledgeOwnerDiagnostic(session, parsed.value);
    }
    const launchFailure = classifyNativeLaunchFailure(line);
    if (launchFailure !== null) this.nativeLaunchFailure ??= launchFailure;
    const pasteFailure = nativePasteFailureCategory(line);
    if (pasteFailure !== null) console.error('Native paste failure:', pasteFailure);
    stderrLine = '';
    discardOversizedLine = false;
  };
  const consumeStderr = (decoded: string): void => {
    for (const fragment of decoded.split(/(\n)/u)) {
      if (fragment === '\n') {
        acceptStderrLine();
        continue;
      }
      if (discardOversizedLine) continue;
      stderrLine += fragment;
      if (Buffer.byteLength(stderrLine) > MAX_TERMINAL_OBSERVABILITY_LINE_BYTES) {
        stderrLine = '';
        discardOversizedLine = true;
      }
    }
  };
  const finishStderr = (): void => {
    if (stderrDrain.completed) return;
    consumeStderr(stderrDecoder.end());
    if (stderrLine !== '' || discardOversizedLine) acceptStderrLine();
    stderrDrain.complete();
  };
  child.stderr.once('error', finishStderr);
  child.stderr.once('end', finishStderr);
  child.stderr.once('close', finishStderr);
  child.stderr.on('data', (chunk: Buffer | string) => {
    consumeStderr(typeof chunk === 'string' ? chunk : stderrDecoder.write(Buffer.from(chunk)));
  });
  child.once('error', () => {
    if (this.child === child && this.rpcSession === session) {
      this.terminateCurrent('spawn-failed', true);
    }
  });
  child.once('exit', () => {
    // A descendant can retain an inherited pipe after the helper has died.
    // Give trailing diagnostics a short drain window, then close our pipe
    // endpoints so the child close event can complete supervision/restart.
    const drainTimer = setTimeout(() => {
      child.stdin.destroy();
      child.stdout.destroy();
      child.stderr.destroy();
    }, STARTUP_STDERR_DRAIN_MS);
    drainTimer.unref();
    child.once('close', () => clearTimeout(drainTimer));
  });
  child.once('close', (code: number | null, signal: NodeJS.Signals | null) => {
    stderrDrain.complete();
    this.publishProcessLifecycle({
      phase: 'exited',
      exitCode: code,
      signal,
      planned: this.plannedExit?.lifecycle === true || !this.desiredRunning,
    });
    const exitDiagnostic = safeChildExitDiagnostic(code, signal);
    if (this.launching !== null && this.nativeLaunchFailure === null) {
      this.nativeLaunchFailure = exitDiagnostic;
    }
    const expectedOutcome = code === 0 && signal === null ? 'shutdown' : 'failure';
    if (
      !duplicateTerminalCandidate &&
      terminalCandidate !== null &&
      terminalCandidate.outcome === expectedOutcome
    ) {
      this.publishRuntimeObservability(terminalCandidate.observability, terminalCandidate.outcome);
    }
    this.handleClose(child, session, generation);
  });
}

export async function waitForStartupStderr(
  this: HelperClientRuntime,
  generation: number,
): Promise<void> {
  const drain = this.stderrDrain;
  if (drain?.generation !== generation || drain.completed) return;
  await waitForCloseWithin(drain.promise, STARTUP_STDERR_DRAIN_MS);
}

export function parseDiagnosticLine(
  this: HelperClientRuntime,
  line: string,
):
  | { readonly kind: 'terminal'; readonly value: HelperTerminalObservabilityRecord }
  | { readonly kind: 'owner-connection'; readonly value: HelperOwnerConnectionDiagnostic }
  | null {
  let candidate: unknown;
  try {
    candidate = JSON.parse(line);
  } catch {
    return null;
  }
  const terminal = HelperTerminalObservabilityRecordSchema.safeParse(candidate);
  if (terminal.success) return { kind: 'terminal', value: terminal.data };
  const ownerConnection = HelperOwnerConnectionDiagnosticSchema.safeParse(candidate);
  return ownerConnection.success ? { kind: 'owner-connection', value: ownerConnection.data } : null;
}

export function commitAndAcknowledgeOwnerDiagnostic(
  this: HelperClientRuntime,
  session: HelperRpcSession,
  diagnostic: HelperOwnerConnectionDiagnostic,
): void {
  const observer = this.options.observeOwnerConnectionDiagnostic;
  if (observer === undefined || !this.rpcChannel.isCurrent(session)) return;
  const dimensions = {
    category: diagnostic.category,
    operation: diagnostic.operation,
    correlationStatus: diagnostic.correlationStatus,
    healthRefresh: diagnostic.healthRefresh,
    transportStatus: diagnostic.transportStatus,
    ownerProcessState: diagnostic.ownerProcessState,
  } as const;
  const key = JSON.stringify({
    journalId: diagnostic.journalId,
    journalNonce: diagnostic.journalNonce,
    dimensions,
    count: diagnostic.count,
  });
  if (this.diagnosticAcksInFlight.has(key)) return;
  if (this.diagnosticAcksInFlight.size >= MAX_DIAGNOSTIC_ACKS_IN_FLIGHT) return;
  const operation = Promise.resolve()
    .then(() => observer(diagnostic))
    .then(async (committed) => {
      if (!committed || !this.rpcChannel.isCurrent(session)) return;
      await this.rpcChannel.request(
        session,
        'diagnostic.ack',
        {
          journalId: diagnostic.journalId,
          journalNonce: diagnostic.journalNonce,
          dimensions,
          count: diagnostic.count,
        },
        {
          timeoutMs: REQUEST_TIMEOUT_MS,
          timeoutReason: 'request-timeout',
          allowDraining: false,
          supervision: false,
          priority: false,
        },
      );
    })
    .catch(() => undefined)
    .finally(() => this.diagnosticAcksInFlight.delete(key));
  this.diagnosticAcksInFlight.set(key, operation);
}

export function publishProcessLifecycle(
  this: HelperClientRuntime,
  event: {
    readonly phase: 'started' | 'exited';
    readonly exitCode?: number | null;
    readonly signal?: NodeJS.Signals | null;
    readonly planned?: boolean;
  },
): void {
  try {
    void Promise.resolve(this.options.observeProcessLifecycle?.(event)).catch(() => undefined);
  } catch {
    // Diagnostics cannot affect native supervision.
  }
}

export function publishRuntimeObservability(
  this: HelperClientRuntime,
  observability: HelperRuntimeObservability,
  source: HelperRuntimeObservabilitySource,
): void {
  const observer = this.options.observeRuntimeObservability;
  if (observer === undefined) return;
  try {
    void Promise.resolve(observer(observability, source)).catch(() => undefined);
  } catch {
    // Diagnostics cannot affect native supervision.
  }
}

export function saturatingSafeIncrement(value: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, value + 1);
}
