import type { MacosFinalizerChild } from './macos-owner-update-coordinator';
import {
  awaitAuthenticatedRollbackOrTerminate,
  boundedTerminate,
  waitForAuthenticatedStatus,
  waitForTerminalExit,
} from './macos-finalizer-status';

const HEX_32 = /^[0-9a-f]{64}$/u;
// Native recovery can spend 30s acquiring exclusion plus 90s restoring and probing.
const NATIVE_RECOVERY_SUPERVISION_MS = 150_000;
// Exceeds the native finalizer's single 120-second absolute uninstall budget.
const UNINSTALL_TERMINAL_SUPERVISION_MS = 150_000;

export async function handoffFinalizer(
  spawnChild: () => MacosFinalizerChild,
  ownerHandoff: string,
  transaction: string,
  uninstallCommit?: () => Promise<void>,
): Promise<void> {
  if (!HEX_32.test(ownerHandoff) || !HEX_32.test(transaction))
    throw new Error('The owner maintenance handoff is invalid');
  let child: MacosFinalizerChild;
  try {
    child = spawnChild();
  } catch (error: unknown) {
    throw new Error('The native maintenance coordinator failed to spawn', { cause: error });
  }
  const extraPipes = child.stdio as (
    NodeJS.ReadableStream | NodeJS.WritableStream | null | undefined
  )[];
  const handoffPipe = extraPipes[3];
  const statusPipe = extraPipes[4];
  const cancellationPipe = extraPipes[5];
  if (
    handoffPipe === null ||
    handoffPipe === undefined ||
    !('end' in handoffPipe) ||
    statusPipe === null ||
    statusPipe === undefined ||
    !('read' in statusPipe) ||
    cancellationPipe === null ||
    cancellationPipe === undefined ||
    !('end' in cancellationPipe)
  ) {
    if (cancellationPipe !== null && cancellationPipe !== undefined && 'end' in cancellationPipe)
      cancellationPipe.end();
    await boundedTerminate(child, 5_000);
    throw new Error('The native maintenance supervision pipes are unavailable');
  }
  handoffPipe.end(Buffer.from(ownerHandoff, 'hex'));
  if (uninstallCommit !== undefined) {
    await superviseUninstallStatus(
      child,
      statusPipe,
      cancellationPipe,
      ownerHandoff,
      transaction,
      uninstallCommit,
      35_000,
      UNINSTALL_TERMINAL_SUPERVISION_MS,
    );
    return;
  }
  const status = await supervisePreReadyStatus(
    child,
    statusPipe,
    cancellationPipe,
    ownerHandoff,
    transaction,
    35_000,
  );
  if (status !== 'ready') {
    cancellationPipe.end();
    await boundedTerminate(child, 10_000);
    throw new Error('The native maintenance coordinator rolled back before handoff');
  }
  const statusControl = statusPipe as NodeJS.ReadableStream & { destroy?: () => void };
  statusControl.destroy?.();
  const unrefCancellation = cancellationPipe as NodeJS.WritableStream & {
    unref?: () => void;
  };
  unrefCancellation.unref?.();
  child.unref();
}

export async function supervisePreReadyStatus(
  child: MacosFinalizerChild,
  statusPipe: NodeJS.ReadableStream,
  cancellationPipe: NodeJS.WritableStream,
  ownerHandoff: string,
  transaction: string,
  timeoutMs: number,
  recoveryTimeoutMs = NATIVE_RECOVERY_SUPERVISION_MS,
): Promise<'ready' | 'error'> {
  try {
    const status = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      timeoutMs,
      ['ready', 'error'],
    );
    return status === 'ready' ? 'ready' : 'error';
  } catch (error: unknown) {
    cancellationPipe.end();
    await awaitAuthenticatedRollbackOrTerminate(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      recoveryTimeoutMs,
    );
    throw error;
  }
}

export async function superviseUninstallStatus(
  child: MacosFinalizerChild,
  statusPipe: NodeJS.ReadableStream,
  cancellationPipe: NodeJS.WritableStream,
  ownerHandoff: string,
  transaction: string,
  removeInstalledApp: () => Promise<void>,
  handoffTimeoutMs: number,
  terminalTimeoutMs: number,
  exitTimeoutMs = 5_000,
  recoveryTimeoutMs = NATIVE_RECOVERY_SUPERVISION_MS,
): Promise<'complete' | 'cleanup_pending'> {
  try {
    const handoff = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      handoffTimeoutMs,
      ['uninstall_ready', 'error'],
    );
    if (handoff === 'error')
      throw new Error('The native uninstall coordinator failed before removal handoff');

    await removeInstalledApp();
    const terminal = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      terminalTimeoutMs,
      ['complete', 'cleanup_pending', 'error'],
    );
    await waitForTerminalExit(child, exitTimeoutMs);
    endWritableOnce(cancellationPipe);
    if (terminal === 'error') throw new Error('The native uninstall coordinator failed closed');
    if (terminal === 'complete') return 'complete';
    return 'cleanup_pending';
  } catch (error: unknown) {
    endWritableOnce(cancellationPipe);
    await awaitAuthenticatedRollbackOrTerminate(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      recoveryTimeoutMs,
    );
    throw error;
  }
}

function endWritableOnce(stream: NodeJS.WritableStream): void {
  const writable = stream as NodeJS.WritableStream & { readonly writableEnded?: boolean };
  if (writable.writableEnded !== true) writable.end();
}
