import type { MacosFinalizerChild } from './macos-owner-update-coordinator';
import { createHmac } from 'node:crypto';

type FinalizerStatus = 'ready' | 'uninstall_ready' | 'complete' | 'cleanup_pending' | 'error';

export async function waitForAuthenticatedStatus(
  child: MacosFinalizerChild,
  stream: NodeJS.ReadableStream,
  handoff: string,
  transaction: string,
  cancellation: NodeJS.WritableStream,
  timeoutMs: number,
  acceptedStates: readonly FinalizerStatus[],
): Promise<FinalizerStatus> {
  return new Promise((resolveStatus, reject) => {
    let settled = false;
    let body = '';
    let recordedChildFailure: Error | null = null;
    const finish = (error: Error | null, state?: FinalizerStatus): boolean => {
      if (settled) return true;
      settled = true;
      clearTimeout(timer);
      child.off('error', onError);
      child.off('exit', onExit);
      stream.removeListener('data', onData);
      stream.removeListener('error', onStreamError);
      stream.removeListener('end', onStreamEnd);
      stream.removeListener('close', onStreamClose);
      stream.pause();
      if (error !== null) reject(error);
      else if (state !== undefined) resolveStatus(state);
      return true;
    };
    const onError = () => {
      recordedChildFailure = new Error('The native maintenance coordinator failed to start');
    };
    const onExit = (code: number | null) => {
      recordedChildFailure = new Error(
        `The native maintenance coordinator exited before authenticated status (${String(code)})`,
      );
    };
    const processStatusBytes = (chunk: string | Buffer): boolean => {
      if (settled) return true;
      body += typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      if (Buffer.byteLength(body) > 512)
        return finish(new Error('The native maintenance status exceeded its bound'));
      const newline = body.indexOf('\n');
      if (newline < 0) return false;
      if (newline !== body.length - 1)
        return finish(new Error('The native maintenance status had trailing data'));
      try {
        const wire = JSON.parse(body) as Record<string, unknown>;
        const state = wire.state;
        if (
          wire.version !== 1 ||
          wire.transaction !== transaction ||
          typeof state !== 'string' ||
          !acceptedStates.includes(state as FinalizerStatus) ||
          wire.mac !== finalizerStatusMac(handoff, transaction, state)
        )
          throw new Error('invalid');
        return finish(null, state as FinalizerStatus);
      } catch {
        return finish(new Error('The native maintenance status was not authenticated'));
      }
    };
    const drainBufferedStatus = (): boolean => {
      const drained: Buffer[] = [];
      let buffered = readBufferedStatusChunk(stream);
      while (buffered !== null) {
        drained.push(Buffer.from(buffered));
        buffered = readBufferedStatusChunk(stream);
      }
      return drained.length > 0 && processStatusBytes(Buffer.concat(drained));
    };
    const finishAtStreamBoundary = (boundary: 'end' | 'close') => {
      if (settled) return;
      try {
        if (drainBufferedStatus()) return;
      } catch {
        finish(new Error('The native maintenance status pipe failed'));
        return;
      }
      finish(
        recordedChildFailure ??
          new Error(`The native maintenance status pipe reached ${boundary} without status`),
      );
    };
    const onData = (chunk: string | Buffer) => processStatusBytes(chunk);
    const onStreamError = () => {
      if (settled) return;
      try {
        if (drainBufferedStatus()) return;
      } catch {
        // The original stream failure remains the authoritative error.
      }
      finish(new Error('The native maintenance status pipe failed'));
    };
    const onStreamEnd = () => finishAtStreamBoundary('end');
    const onStreamClose = () => finishAtStreamBoundary('close');
    // Deadline cancellation is cooperative: closing fd 5 instructs the native
    // coordinator to finish rollback and re-registration.
    const timer = setTimeout(() => {
      cancellation.end();
      finish(new Error('The native maintenance coordinator exceeded its pre-ready deadline'));
    }, timeoutMs);
    // An exit may already be recorded while its authenticated terminal line is
    // buffered in the pipe. Preserve that ordering evidence before listeners.
    if (childHasExited(child)) {
      recordedChildFailure = new Error(
        'The native maintenance coordinator exited before authenticated status',
      );
    }
    stream.pause();
    child.once('error', onError);
    child.once('exit', onExit);
    stream.on('data', onData);
    stream.once('error', onStreamError);
    stream.once('end', onStreamEnd);
    stream.once('close', onStreamClose);
    // Drain every byte currently buffered while paused, then parse the complete
    // aggregate so a valid line cannot hide buffered trailing/malformed data.
    try {
      if (drainBufferedStatus()) return;
    } catch {
      finish(new Error('The native maintenance status pipe failed'));
      return;
    }
    // Child exit is only recorded. The pipe may still contain or later deliver
    // the required authenticated line before its writer reaches EOF.
    if (childHasExited(child) && recordedChildFailure === null) {
      recordedChildFailure = new Error(
        'The native maintenance coordinator exited before authenticated status',
      );
    }
    stream.resume();
    // Readers remain installed until authenticated status, exit, stream error,
    // or deadline; every path converges on finish() exactly once.
  });
}

function readBufferedStatusChunk(stream: NodeJS.ReadableStream): string | Buffer | null {
  const value: unknown = stream.read();
  if (value === null || typeof value === 'string' || Buffer.isBuffer(value)) return value;
  throw new Error('The native maintenance status pipe returned invalid buffered data');
}

function finalizerStatusMac(handoff: string, transaction: string, state: string): string {
  return createHmac('sha256', Buffer.from(handoff, 'hex'))
    .update('talking-quill/macos-finalizer-status/v1\0')
    .update(transaction)
    .update(state)
    .digest('hex');
}

export async function awaitAuthenticatedRollbackOrTerminate(
  child: MacosFinalizerChild,
  stream: NodeJS.ReadableStream,
  handoff: string,
  transaction: string,
  timeoutMs: number,
): Promise<void> {
  const authenticated = new Promise<boolean>((resolve) => {
    let body = '';
    let settled = false;
    const finish = (value: boolean) => {
      if (settled) return;
      settled = true;
      stream.removeListener('data', onData);
      child.removeListener('exit', onExit);
      resolve(value);
    };
    const onExit = () => finish(false);
    const onData = (chunk: string | Buffer) => {
      body += typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      if (Buffer.byteLength(body) > 512) return finish(false);
      const newline = body.indexOf('\n');
      if (newline < 0) return;
      try {
        const wire = JSON.parse(body.slice(0, newline + 1)) as Record<string, unknown>;
        finish(
          wire.version === 1 &&
            wire.transaction === transaction &&
            wire.state === 'error' &&
            wire.mac === finalizerStatusMac(handoff, transaction, 'error'),
        );
      } catch {
        finish(false);
      }
    };
    if (childHasExited(child)) {
      finish(false);
      return;
    }
    stream.on('data', onData);
    child.once('exit', onExit);
    // Exit can occur between the first state check and listener installation;
    // Node records it on the ChildProcess even when the event already fired.
    if (childHasExited(child)) finish(false);
  });
  const observed = await Promise.race([
    authenticated,
    new Promise<false>((resolve) => setTimeout(() => resolve(false), timeoutMs)),
  ]);
  if (!observed || (child.exitCode === null && child.signalCode === null)) {
    await boundedTerminate(child, 2_000);
  }
}

function childHasExited(child: MacosFinalizerChild): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

async function waitForChildExit(child: MacosFinalizerChild, timeoutMs: number): Promise<boolean> {
  if (childHasExited(child)) return true;
  return new Promise((resolve) => {
    let settled = false;
    const finish = (exited: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.removeListener('exit', onExit);
      resolve(exited);
    };
    const onExit = () => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);
    child.once('exit', onExit);
    // Close the event-before-listener and event-during-listener-installation races.
    if (childHasExited(child)) finish(true);
  });
}

export async function boundedTerminate(
  child: MacosFinalizerChild,
  timeoutMs: number,
): Promise<void> {
  if (await waitForChildExit(child, timeoutMs)) return;
  if (childHasExited(child)) return;
  child.kill('SIGTERM');
  if (await waitForChildExit(child, 2_000)) return;
  if (childHasExited(child)) return;
  child.kill('SIGKILL');
  await waitForChildExit(child, 2_000);
}

export async function waitForTerminalExit(
  child: MacosFinalizerChild,
  timeoutMs: number,
): Promise<void> {
  const exited = await waitForChildExit(child, timeoutMs);
  if (!exited) {
    await boundedTerminate(child, 2_000);
    throw new Error('The native uninstall coordinator did not exit after terminal status');
  }
}
