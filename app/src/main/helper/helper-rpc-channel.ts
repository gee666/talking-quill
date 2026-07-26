import { performance } from 'node:perf_hooks';
import {
  HelperNotificationSchema,
  HelperRpcResponseSchema,
  helperParamsSchemas,
  helperResultSchemas,
  type HelperMethod,
  type HelperNotification,
  type HelperParams,
  type HelperResult,
} from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { decodeHelperJson, encodeHelperFrame, HelperFrameDecoder } from './framing';

const MAX_OUTSTANDING_REQUESTS = 256;
const RESERVED_SUPERVISION_REQUESTS = 2;

export type HelperRpcErrorCode =
  'not-running' | 'request-capacity' | 'request-timeout' | 'rpc-error' | 'transport-error';

interface HelperRpcStreams {
  readonly stdin: NodeJS.WritableStream & {
    readonly destroyed: boolean;
    readonly writable: boolean;
    write(chunk: Uint8Array, callback: (error?: Error | null) => void): boolean;
  };
  readonly stdout: NodeJS.ReadableStream;
}

export interface HelperRpcSession {
  readonly child: HelperRpcStreams;
  readonly token: symbol;
}

interface PendingRequest {
  readonly method: HelperMethod;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  readonly deadlineAt: number;
  readonly timeoutReason: HelperReadinessReason;
  readonly removeAbort: () => void;
  readonly onPasteCommitted?: (() => void) | undefined;
  readonly allowDraining: boolean;
  readonly supervision: boolean;
  timer: NodeJS.Timeout | null;
  dispatched: boolean;
  abortRequested: boolean;
  pasteCommitted: boolean;
}

interface QueuedWrite {
  readonly id: number;
  readonly frame: Buffer;
  readonly allowDraining: boolean;
}

interface HelperRpcChannelOptions {
  readonly createError: (
    code: HelperRpcErrorCode,
    message: string,
    rpcCode?: number | null,
  ) => Error;
  readonly onFault: (
    session: HelperRpcSession,
    reason: HelperReadinessReason,
    pendingError?: Error,
  ) => void;
  readonly onNotification: (session: HelperRpcSession, notification: HelperNotification) => void;
}

interface HelperRpcRequestOptions {
  readonly timeoutMs: number;
  readonly timeoutReason: HelperReadinessReason;
  readonly signal?: AbortSignal | undefined;
  readonly onPasteCommitted?: (() => void) | undefined;
  readonly allowDraining: boolean;
  readonly supervision: boolean;
}

/** Internal transport for the one helper process currently owned by HelperClient. */
export class HelperRpcChannel {
  readonly #options: HelperRpcChannelOptions;
  readonly #pending = new Map<number, PendingRequest>();
  readonly #ignoredResponseIds = new Set<number>();
  readonly #writeQueue: QueuedWrite[] = [];
  #session: HelperRpcSession | null = null;
  #decoder = new HelperFrameDecoder();
  #writeBlocked = false;
  #writeClosed = true;
  #draining = false;
  #nextRequestId = 1;

  constructor(options: HelperRpcChannelOptions) {
    this.#options = options;
  }

  attach(child: HelperRpcStreams): HelperRpcSession {
    if (this.#session !== null) {
      throw this.#options.createError(
        'transport-error',
        'Native helper RPC channel is already attached',
      );
    }
    const session = Object.freeze({ child, token: Symbol('helper-rpc-session') });
    this.#session = session;
    this.#decoder = new HelperFrameDecoder();
    this.#ignoredResponseIds.clear();
    this.#writeQueue.length = 0;
    this.#writeBlocked = false;
    this.#writeClosed = false;
    this.#draining = false;
    this.#attachStreams(session);
    return session;
  }

  isCurrent(session: HelperRpcSession): boolean {
    return this.#session === session && !this.#writeClosed;
  }

  beginDraining(session: HelperRpcSession): void {
    if (this.#session !== session || this.#draining) return;
    this.#draining = true;
    const error = this.#options.createError('not-running', 'Native helper is terminating');
    for (let index = this.#writeQueue.length - 1; index >= 0; index -= 1) {
      const queued = this.#writeQueue[index];
      if (queued === undefined || queued.allowDraining) continue;
      this.#writeQueue.splice(index, 1);
      this.#rejectQueuedRequest(queued.id, error);
    }
  }

  request<Method extends HelperMethod>(
    session: HelperRpcSession,
    method: Method,
    params: HelperParams<Method>,
    options: HelperRpcRequestOptions,
  ): Promise<HelperResult<Method>> {
    if (options.signal?.aborted === true) {
      return Promise.reject(new DOMException('Native helper request cancelled', 'AbortError'));
    }
    const child = session.child;
    if (
      this.#session !== session ||
      this.#writeClosed ||
      (this.#draining && !options.allowDraining) ||
      child.stdin.destroyed ||
      !child.stdin.writable
    ) {
      return Promise.reject(
        this.#options.createError('not-running', 'Native helper is terminating'),
      );
    }

    const requestCapacity = options.supervision
      ? MAX_OUTSTANDING_REQUESTS
      : MAX_OUTSTANDING_REQUESTS - RESERVED_SUPERVISION_REQUESTS;
    if (this.#pending.size >= requestCapacity) {
      return Promise.reject(
        this.#options.createError('request-capacity', 'Native helper request capacity is full'),
      );
    }

    const id = this.#takeRequestId();
    const validParams = helperParamsSchemas[method].parse(params);
    const frame = encodeHelperFrame({ jsonrpc: '2.0', id, method, params: validParams });
    return new Promise<HelperResult<Method>>((resolve, reject) => {
      const abort = (): void => {
        if (this.#session !== session) return;
        const pending = this.#pending.get(id);
        if (pending === undefined) return;
        if (pending.dispatched) {
          pending.abortRequested = true;
          return;
        }
        const queuedIndex = this.#writeQueue.findIndex((queued) => queued.id === id);
        if (queuedIndex !== -1) this.#writeQueue.splice(queuedIndex, 1);
        if (pending.timer !== null) clearTimeout(pending.timer);
        pending.removeAbort();
        this.#pending.delete(id);
        pending.reject(new DOMException('Native helper request cancelled', 'AbortError'));
      };
      options.signal?.addEventListener('abort', abort, { once: true });
      this.#pending.set(id, {
        method,
        deadlineAt: performance.now() + options.timeoutMs,
        timeoutReason: options.timeoutReason,
        timer: null,
        dispatched: false,
        resolve: (result) => resolve(result as HelperResult<Method>),
        reject,
        removeAbort: () => options.signal?.removeEventListener('abort', abort),
        onPasteCommitted: options.onPasteCommitted,
        allowDraining: options.allowDraining,
        supervision: options.supervision,
        abortRequested: false,
        pasteCommitted: false,
      });
      this.#armRequestTimeout(session, id);
      this.#writeQueue.push({ id, frame, allowDraining: options.allowDraining });
      this.#pumpWrites(session);
    });
  }

  close(session: HelperRpcSession, pendingError: Error): void {
    if (this.#session !== session) return;
    this.#writeClosed = true;
    this.#draining = true;
    this.#writeBlocked = false;
    this.#writeQueue.length = 0;
    this.#ignoredResponseIds.clear();
    this.#rejectPending(pendingError);
    this.#session = null;
  }

  #attachStreams(session: HelperRpcSession): void {
    const child = session.child;
    child.stdin.on('drain', () => {
      if (!this.isCurrent(session)) return;
      this.#writeBlocked = false;
      this.#pumpWrites(session);
    });
    child.stdin.once('error', () => this.#failTransport(session, 'Native helper stdin failed'));
    child.stdin.once('close', () => this.#failTransport(session, 'Native helper stdin closed'));
    child.stdout.once('error', () => this.#failTransport(session, 'Native helper stdout failed'));
    child.stdout.on('data', (chunk: Buffer) => {
      if (!this.isCurrent(session)) return;
      try {
        for (const payload of this.#decoder.push(chunk)) this.#acceptPayload(session, payload);
      } catch {
        this.#options.onFault(session, 'malformed-response');
      }
    });
    child.stdout.once('end', () => {
      if (!this.isCurrent(session)) return;
      try {
        this.#decoder.finish();
      } catch {
        this.#options.onFault(session, 'malformed-response');
        return;
      }
      this.#options.onFault(session, 'unexpected-exit');
    });
  }

  #armRequestTimeout(session: HelperRpcSession, id: number): void {
    const pending = this.#pending.get(id);
    if (pending === undefined) return;
    if (pending.timer !== null) clearTimeout(pending.timer);
    const remaining = Math.max(0, pending.deadlineAt - performance.now());
    pending.timer = setTimeout(() => this.#expireRequest(session, id), remaining);
    pending.timer.unref();
  }

  #expireRequest(session: HelperRpcSession, id: number): void {
    if (this.#session !== session) return;
    const current = this.#pending.get(id);
    if (current === undefined) return;
    const queuedIndex = this.#writeQueue.findIndex((queued) => queued.id === id);
    if (queuedIndex !== -1) this.#writeQueue.splice(queuedIndex, 1);
    if (current.timer !== null) clearTimeout(current.timer);
    current.removeAbort();
    this.#pending.delete(id);
    const drainingSupervision = this.#draining && current.supervision && !current.allowDraining;
    if (drainingSupervision) this.#ignoredResponseIds.add(id);
    current.reject(
      current.abortRequested
        ? new DOMException('Native helper request cancelled', 'AbortError')
        : this.#options.createError('request-timeout', `Native helper ${current.method} timed out`),
    );
    if (!drainingSupervision) this.#options.onFault(session, current.timeoutReason);
  }

  #pumpWrites(session: HelperRpcSession): void {
    if (!this.isCurrent(session) || this.#writeBlocked) return;
    const child = session.child;
    while (this.#writeQueue.length > 0) {
      const queued = this.#writeQueue.shift();
      if (queued === undefined) return;
      const pending = this.#pending.get(queued.id);
      if (pending === undefined) continue;
      if (child.stdin.destroyed || !child.stdin.writable) {
        this.#failTransport(session, 'Native helper stdin is unavailable');
        return;
      }
      if (performance.now() >= pending.deadlineAt) {
        this.#expireRequest(session, queued.id);
        return;
      }

      pending.dispatched = true;
      const writable = child.stdin.write(queued.frame, (error) => {
        if (error !== null && error !== undefined) {
          this.#failTransport(session, 'Native helper stdin failed');
        }
      });
      if (!writable) {
        this.#writeBlocked = true;
        return;
      }
    }
  }

  #failTransport(session: HelperRpcSession, message: string): void {
    if (!this.isCurrent(session)) return;
    this.#options.onFault(
      session,
      'unexpected-exit',
      this.#options.createError('transport-error', message),
    );
  }

  #acceptPayload(session: HelperRpcSession, payload: Buffer): void {
    if (!this.isCurrent(session)) return;
    const raw = decodeHelperJson(payload);
    const response = HelperRpcResponseSchema.safeParse(raw);
    if (response.success) {
      if (response.data.id === null) throw new Error('Uncorrelated helper response');
      if (this.#ignoredResponseIds.delete(response.data.id)) return;
      const pending = this.#pending.get(response.data.id);
      if (pending === undefined) throw new Error('Unknown helper response ID');
      if (pending.timer !== null) clearTimeout(pending.timer);
      pending.removeAbort();
      this.#pending.delete(response.data.id);
      if ('error' in response.data) {
        pending.reject(
          this.#options.createError(
            'rpc-error',
            `Native helper rejected ${pending.method}`,
            response.data.error.code,
          ),
        );
        return;
      }
      const result = helperResultSchemas[pending.method].safeParse(response.data.result);
      if (!result.success) {
        pending.reject(
          this.#options.createError('transport-error', 'Invalid helper result schema'),
        );
        throw new Error('Invalid helper result schema');
      }
      pending.resolve(result.data);
      return;
    }

    const notification = HelperNotificationSchema.parse(raw);
    if (notification.method === 'paste.committed') {
      const pending = this.#pending.get(notification.params.requestId);
      if (pending?.method !== 'paste.inject') throw new Error('Unknown paste commit request ID');
      if (pending.pasteCommitted) return;
      pending.pasteCommitted = true;
      try {
        pending.onPasteCommitted?.();
      } catch {
        // Commit observers are application callbacks, not part of protocol supervision.
      }
      return;
    }
    this.#options.onNotification(session, notification);
  }

  #takeRequestId(): number {
    const id = this.#nextRequestId;
    this.#nextRequestId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    if (this.#pending.has(id) || this.#ignoredResponseIds.has(id)) {
      throw this.#options.createError('transport-error', 'Request ID exhausted');
    }
    return id;
  }

  #rejectQueuedRequest(id: number, error: Error): void {
    const pending = this.#pending.get(id);
    if (pending === undefined || pending.dispatched) return;
    if (pending.timer !== null) clearTimeout(pending.timer);
    pending.removeAbort();
    this.#pending.delete(id);
    pending.reject(error);
  }

  #rejectPending(error: Error): void {
    for (const pending of this.#pending.values()) {
      if (pending.timer !== null) clearTimeout(pending.timer);
      pending.removeAbort();
      pending.reject(error);
    }
    this.#pending.clear();
  }
}
