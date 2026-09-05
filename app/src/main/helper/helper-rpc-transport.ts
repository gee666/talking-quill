import { performance } from 'node:perf_hooks';
import { type HelperRpcRuntime, type HelperRpcSession } from './helper-rpc-runtime';

export function attachStreams(this: HelperRpcRuntime, session: HelperRpcSession): void {
  const child = session.child;
  child.stdin.on('drain', () => {
    if (!this.isCurrent(session)) return;
    this.writeBlocked = false;
    this.pumpWrites(session);
  });
  child.stdin.once('error', () => this.failTransport(session, 'Native helper stdin failed'));
  child.stdin.once('close', () => this.failTransport(session, 'Native helper stdin closed'));
  child.stdout.once('error', () => this.failTransport(session, 'Native helper stdout failed'));
  child.stdout.on('data', (chunk: Buffer) => {
    if (!this.isCurrent(session)) return;
    try {
      for (const payload of this.decoder.push(chunk)) this.acceptPayload(session, payload);
    } catch (error: unknown) {
      this.options.onFault(
        session,
        'malformed-response',
        this.options.createError(
          'transport-error',
          error instanceof Error ? error.message : 'Malformed helper response',
        ),
      );
    }
  });
  child.stdout.once('end', () => {
    if (!this.isCurrent(session)) return;
    try {
      this.decoder.finish();
    } catch {
      // EOF can split an otherwise valid frame at any byte when the helper
      // crashes. Complete invalid frames are rejected in push() above;
      // truncated EOF follows bounded crash supervision instead.
      this.options.onFault(session, 'unexpected-exit');
      return;
    }
    this.options.onFault(session, 'unexpected-exit');
  });
}

export function armRequestTimeout(
  this: HelperRpcRuntime,
  session: HelperRpcSession,
  id: number,
): void {
  const pending = this.pending.get(id);
  if (pending === undefined) return;
  const deadlineAt = pending.deadlineAt;
  if (deadlineAt === null) return;
  if (pending.timer !== null) clearTimeout(pending.timer);
  const remaining = Math.max(0, deadlineAt - performance.now());
  pending.timer = setTimeout(() => this.expireRequest(session, id), remaining);
  pending.timer.unref();
}

export function expireRequest(this: HelperRpcRuntime, session: HelperRpcSession, id: number): void {
  if (this.session !== session) return;
  const current = this.pending.get(id);
  if (current === undefined) return;
  const queuedIndex = this.writeQueue.findIndex((queued) => queued.id === id);
  if (queuedIndex !== -1) this.writeQueue.splice(queuedIndex, 1);
  if (current.timer !== null) clearTimeout(current.timer);
  current.removeAbort();
  this.pending.delete(id);
  const ignoredDispatchedPredecessor =
    this.draining && current.dispatched && !current.allowDraining;
  const nonSupervisingDiagnostic = current.method === 'diagnostic.ack';
  if (ignoredDispatchedPredecessor || (current.dispatched && nonSupervisingDiagnostic)) {
    this.ignoredResponseIds.add(id);
  }
  current.reject(
    current.abortRequested
      ? new DOMException('Native helper request cancelled', 'AbortError')
      : this.options.createError('request-timeout', `Native helper ${current.method} timed out`),
  );
  if (current.dispatched && this.dispatchedId === id) {
    this.dispatchedId = null;
  }
  if (ignoredDispatchedPredecessor || nonSupervisingDiagnostic) {
    // Diagnostic acknowledgements never supervise the shared process. A late
    // response is ignored and the durable helper journal will replay.
    this.pumpWrites(session);
  } else {
    // Establish fault/drain policy before any queued mutation can dispatch.
    this.options.onFault(session, current.timeoutReason);
  }
}

export function pumpWrites(this: HelperRpcRuntime, session: HelperRpcSession): void {
  if (!this.isCurrent(session) || this.writeBlocked || this.dispatchedId !== null) return;
  const child = session.child;
  while (this.writeQueue.length > 0) {
    const queued = this.writeQueue.shift();
    if (queued === undefined) return;
    const pending = this.pending.get(queued.id);
    if (pending === undefined) continue;
    if (child.stdin.destroyed || !child.stdin.writable) {
      this.failTransport(session, 'Native helper stdin is unavailable');
      return;
    }
    if (pending.deadlineAt !== null && performance.now() >= pending.deadlineAt) {
      this.expireRequest(session, queued.id);
      return;
    }

    pending.dispatched = true;
    this.dispatchedId = queued.id;
    let writable: boolean;
    try {
      writable = child.stdin.write(queued.frame, (error) => {
        if (error !== null && error !== undefined) {
          this.failTransport(session, 'Native helper stdin failed');
        }
      });
    } catch {
      this.failTransport(session, 'Native helper stdin failed');
      return;
    }
    if (pending.timeoutStartsOnDispatch && this.pending.has(queued.id)) {
      pending.deadlineAt = performance.now() + pending.timeoutMs;
      this.armRequestTimeout(session, queued.id);
    }
    try {
      pending.onDispatched?.();
    } catch {
      // Dispatch observation belongs to HelperClient supervision, not protocol parsing.
    }
    if (!writable) this.writeBlocked = true;
    return;
  }
}

export function failTransport(
  this: HelperRpcRuntime,
  session: HelperRpcSession,
  message: string,
): void {
  if (!this.isCurrent(session)) return;
  this.options.onFault(
    session,
    'unexpected-exit',
    this.options.createError('transport-error', message),
  );
}

export function takeRequestId(this: HelperRpcRuntime): number {
  const id = this.nextRequestId;
  this.nextRequestId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
  if (this.pending.has(id) || this.ignoredResponseIds.has(id)) {
    throw this.options.createError('transport-error', 'Request ID exhausted');
  }
  return id;
}

export function rejectQueuedRequest(this: HelperRpcRuntime, id: number, error: Error): void {
  const pending = this.pending.get(id);
  if (pending === undefined || pending.dispatched) return;
  if (pending.timer !== null) clearTimeout(pending.timer);
  pending.removeAbort();
  this.pending.delete(id);
  pending.reject(error);
}

export function rejectPending(this: HelperRpcRuntime, error: Error): void {
  for (const pending of this.pending.values()) {
    if (pending.timer !== null) clearTimeout(pending.timer);
    pending.removeAbort();
    pending.reject(error);
  }
  this.pending.clear();
}
