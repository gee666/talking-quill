import { performance } from 'node:perf_hooks';
import { z, type ZodType } from 'zod';
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
import { shortcutsEqual } from '../../shared/schemas/shortcut';
import { decodeHelperJson, encodeHelperFrame, HelperFrameDecoder } from './framing';

const MAX_ORDINARY_REQUESTS = 256;
const RESERVED_SHUTDOWN_REQUESTS = 1;
const MAX_DEFERRED_ACTIVATION_EVENTS = 16;

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

type ActivationNotification = Extract<HelperNotification, { method: 'activation.event' }>;
type PairedActivationParams = Extract<ActivationNotification['params'], { phase: 'down' | 'up' }>;

interface PendingRequest {
  readonly method: string;
  readonly resultSchema: ZodType;
  readonly activationConfiguration: HelperParams<'activation.configure'> | null;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  readonly timeoutMs: number;
  deadlineAt: number | null;
  readonly timeoutReason: HelperReadinessReason;
  readonly removeAbort: () => void;
  readonly onPasteCommitted?: (() => void) | undefined;
  readonly allowDraining: boolean;
  readonly supervision: boolean;
  readonly timeoutStartsOnDispatch: boolean;
  readonly onDispatched?: (() => void) | undefined;
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
  readonly onPingResult?: (session: HelperRpcSession, result: HelperResult<'ping'>) => void;
}

interface HelperRpcRequestOptions {
  readonly timeoutMs: number;
  readonly timeoutReason: HelperReadinessReason;
  readonly signal?: AbortSignal | undefined;
  readonly onPasteCommitted?: (() => void) | undefined;
  readonly allowDraining: boolean;
  readonly supervision: boolean;
  readonly timeoutStartsOnDispatch?: boolean | undefined;
  readonly onDispatched?: (() => void) | undefined;
  readonly priority?: boolean | undefined;
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
  #dispatchedId: number | null = null;
  #draining = false;
  #nextRequestId = 1;
  #lastActivationGeneration = 0;
  #activeActivation: PairedActivationParams | null = null;
  #deferredActivationEvents: {
    readonly requestId: number;
    readonly notifications: ActivationNotification[];
  } | null = null;

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
    this.#dispatchedId = null;
    this.#draining = false;
    this.#lastActivationGeneration = 0;
    this.#activeActivation = null;
    this.#deferredActivationEvents = null;
    this.#attachStreams(session);
    return session;
  }

  isCurrent(session: HelperRpcSession): boolean {
    return this.#session === session && !this.#writeClosed;
  }

  resetOwnerActivationStream(session: HelperRpcSession): void {
    if (this.#session !== session) return;
    this.#lastActivationGeneration = 0;
    this.#activeActivation = null;
    this.#deferredActivationEvents = null;
  }

  beginDraining(session: HelperRpcSession): void {
    if (this.#session !== session) return;
    if (!this.#draining) {
      this.#draining = true;
      const error = this.#options.createError('not-running', 'Native helper is terminating');
      for (let index = this.#writeQueue.length - 1; index >= 0; index -= 1) {
        const queued = this.#writeQueue[index];
        if (queued === undefined || queued.allowDraining) continue;
        this.#writeQueue.splice(index, 1);
        this.#rejectQueuedRequest(queued.id, error);
      }
    }
    // A correlated protocol fault releases dispatched authority without
    // pumping. The fault supervisor calls this method again only after it has
    // rejected ordinary work and deliberately admitted reserved shutdown.
    this.#pumpWrites(session);
  }

  request<Method extends HelperMethod>(
    session: HelperRpcSession,
    method: Method,
    params: HelperParams<Method>,
    options: HelperRpcRequestOptions,
  ): Promise<HelperResult<Method>> {
    return this.#request(
      session,
      method,
      params,
      helperParamsSchemas[method],
      helperResultSchemas[method],
      options,
    ) as Promise<HelperResult<Method>>;
  }

  requestExtension(
    session: HelperRpcSession,
    method: string,
    resultSchema: ZodType,
    options: HelperRpcRequestOptions,
  ): Promise<unknown> {
    return this.#request(session, method, {}, z.object({}).strict(), resultSchema, options);
  }

  #request(
    session: HelperRpcSession,
    method: string,
    params: unknown,
    paramsSchema: ZodType,
    resultSchema: ZodType,
    options: HelperRpcRequestOptions,
  ): Promise<unknown> {
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

    const requestCapacity = options.allowDraining
      ? MAX_ORDINARY_REQUESTS + RESERVED_SHUTDOWN_REQUESTS
      : MAX_ORDINARY_REQUESTS;
    if (this.#pending.size >= requestCapacity) {
      return Promise.reject(
        this.#options.createError('request-capacity', 'Native helper request capacity is full'),
      );
    }

    const id = this.#takeRequestId();
    const validParams = paramsSchema.parse(params);
    const frame = encodeHelperFrame({ jsonrpc: '2.0', id, method, params: validParams });
    return new Promise<unknown>((resolve, reject) => {
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
        resultSchema,
        activationConfiguration:
          method === 'activation.configure'
            ? helperParamsSchemas['activation.configure'].parse(validParams)
            : null,
        timeoutMs: options.timeoutMs,
        deadlineAt:
          options.timeoutStartsOnDispatch === true ? null : performance.now() + options.timeoutMs,
        timeoutReason: options.timeoutReason,
        timer: null,
        dispatched: false,
        resolve,
        reject,
        removeAbort: () => options.signal?.removeEventListener('abort', abort),
        onPasteCommitted: options.onPasteCommitted,
        allowDraining: options.allowDraining,
        supervision: options.supervision,
        timeoutStartsOnDispatch: options.timeoutStartsOnDispatch === true,
        onDispatched: options.onDispatched,
        abortRequested: false,
        pasteCommitted: false,
      });
      this.#armRequestTimeout(session, id);
      const queued = { id, frame, allowDraining: options.allowDraining };
      if (options.allowDraining || options.priority === true) this.#writeQueue.unshift(queued);
      else this.#writeQueue.push(queued);
      this.#pumpWrites(session);
    });
  }

  close(session: HelperRpcSession, pendingError: Error): void {
    if (this.#session !== session) return;
    this.#writeClosed = true;
    this.#draining = true;
    this.#writeBlocked = false;
    this.#dispatchedId = null;
    this.#writeQueue.length = 0;
    this.#ignoredResponseIds.clear();
    this.#lastActivationGeneration = 0;
    this.#activeActivation = null;
    this.#deferredActivationEvents = null;
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
      } catch (error: unknown) {
        this.#options.onFault(
          session,
          'malformed-response',
          this.#options.createError(
            'transport-error',
            error instanceof Error ? error.message : 'Malformed helper response',
          ),
        );
      }
    });
    child.stdout.once('end', () => {
      if (!this.isCurrent(session)) return;
      try {
        this.#decoder.finish();
      } catch {
        // EOF can split an otherwise valid frame at any byte when the helper
        // crashes. Complete invalid frames are rejected in push() above;
        // truncated EOF follows bounded crash supervision instead.
        this.#options.onFault(session, 'unexpected-exit');
        return;
      }
      this.#options.onFault(session, 'unexpected-exit');
    });
  }

  #armRequestTimeout(session: HelperRpcSession, id: number): void {
    const pending = this.#pending.get(id);
    if (pending === undefined) return;
    const deadlineAt = pending.deadlineAt;
    if (deadlineAt === null) return;
    if (pending.timer !== null) clearTimeout(pending.timer);
    const remaining = Math.max(0, deadlineAt - performance.now());
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
    const ignoredDispatchedPredecessor =
      this.#draining && current.dispatched && !current.allowDraining;
    const nonSupervisingDiagnostic = current.method === 'diagnostic.ack';
    if (ignoredDispatchedPredecessor || (current.dispatched && nonSupervisingDiagnostic)) {
      this.#ignoredResponseIds.add(id);
    }
    current.reject(
      current.abortRequested
        ? new DOMException('Native helper request cancelled', 'AbortError')
        : this.#options.createError('request-timeout', `Native helper ${current.method} timed out`),
    );
    if (current.dispatched && this.#dispatchedId === id) {
      this.#dispatchedId = null;
    }
    if (ignoredDispatchedPredecessor || nonSupervisingDiagnostic) {
      // Diagnostic acknowledgements never supervise the shared process. A late
      // response is ignored and the durable helper journal will replay.
      this.#pumpWrites(session);
    } else {
      // Establish fault/drain policy before any queued mutation can dispatch.
      this.#options.onFault(session, current.timeoutReason);
    }
  }

  #pumpWrites(session: HelperRpcSession): void {
    if (!this.isCurrent(session) || this.#writeBlocked || this.#dispatchedId !== null) return;
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
      if (pending.deadlineAt !== null && performance.now() >= pending.deadlineAt) {
        this.#expireRequest(session, queued.id);
        return;
      }

      pending.dispatched = true;
      this.#dispatchedId = queued.id;
      let writable: boolean;
      try {
        writable = child.stdin.write(queued.frame, (error) => {
          if (error !== null && error !== undefined) {
            this.#failTransport(session, 'Native helper stdin failed');
          }
        });
      } catch {
        this.#failTransport(session, 'Native helper stdin failed');
        return;
      }
      if (pending.timeoutStartsOnDispatch && this.#pending.has(queued.id)) {
        pending.deadlineAt = performance.now() + pending.timeoutMs;
        this.#armRequestTimeout(session, queued.id);
      }
      try {
        pending.onDispatched?.();
      } catch {
        // Dispatch observation belongs to HelperClient supervision, not protocol parsing.
      }
      if (!writable) this.#writeBlocked = true;
      return;
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
      if (typeof response.data.id !== 'number') {
        throw new Error('Uncorrelated helper response');
      }
      if (this.#ignoredResponseIds.delete(response.data.id)) return;
      const pending = this.#pending.get(response.data.id);
      if (pending === undefined || !pending.dispatched || this.#dispatchedId !== response.data.id) {
        throw new Error('Helper response does not match dispatched authority');
      }
      if ('error' in response.data) {
        if (this.#deferredActivationEvents?.requestId === response.data.id) {
          this.#deferredActivationEvents = null;
        }
        this.#releaseDispatchedRequest(response.data.id, pending);
        pending.reject(
          this.#options.createError(
            'rpc-error',
            `Native helper rejected ${pending.method}`,
            response.data.error.code,
          ),
        );
        this.#pumpWrites(session);
        return;
      }
      const result = pending.resultSchema.safeParse(response.data.result);
      if (!result.success) {
        this.#rejectMalformedDispatchedResponse(
          response.data.id,
          pending,
          'Invalid helper result schema',
        );
      }
      if (pending.method === 'paste.inject') {
        const pasteSubmitted = helperResultSchemas['paste.inject'].parse(result.data).submitted;
        if (pasteSubmitted !== pending.pasteCommitted) {
          this.#rejectMalformedDispatchedResponse(
            response.data.id,
            pending,
            pasteSubmitted
              ? 'Native helper acknowledged paste before its commit notification'
              : 'Native helper committed a paste before reporting rejection',
          );
        }
      }
      if (pending.method === 'ping') {
        this.#options.onPingResult?.(session, helperResultSchemas.ping.parse(result.data));
      }
      if (pending.method === 'activation.configure') {
        const requested = pending.activationConfiguration;
        const effective = helperResultSchemas['activation.configure'].parse(result.data);
        const deferred = this.#deferredActivationEvents;
        if (deferred?.requestId === response.data.id) this.#deferredActivationEvents = null;
        if (requested !== null && activationAcknowledgementMatches(requested, effective)) {
          // Configuration replacement cancels the old pair. Its response can race with newly
          // admitted event output, so validate and publish events held behind that response now.
          this.#activeActivation = null;
          for (const event of deferred?.notifications ?? []) {
            this.#validateActivationEvent(event);
            this.#options.onNotification(session, event);
          }
        }
      }
      // Method-specific result and paste ordering are now authoritative. Only
      // this point may release the serialized slot and expose its successor.
      this.#releaseDispatchedRequest(response.data.id, pending);
      pending.resolve(result.data);
      this.#pumpWrites(session);
      return;
    }

    const notification = HelperNotificationSchema.parse(raw);
    if (notification.method === 'activation.event') {
      if (this.#deferActivationEvent(notification)) return;
      this.#validateActivationEvent(notification);
    }
    if (notification.method === 'paste.committed') {
      if (typeof notification.params.requestId !== 'number') {
        throw new Error('Unknown paste commit request ID');
      }
      const pending = this.#pending.get(notification.params.requestId);
      if (
        pending?.method !== 'paste.inject' ||
        !pending.dispatched ||
        this.#dispatchedId !== notification.params.requestId
      ) {
        throw new Error('Unknown paste commit request ID');
      }
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

  #releaseDispatchedRequest(id: number, pending: PendingRequest): void {
    if (pending.timer !== null) clearTimeout(pending.timer);
    pending.removeAbort();
    this.#pending.delete(id);
    if (this.#dispatchedId === id) this.#dispatchedId = null;
  }

  #rejectMalformedDispatchedResponse(id: number, pending: PendingRequest, message: string): never {
    const error = this.#options.createError('transport-error', message);
    // Release only the malformed request. Do not pump here: onFault must first
    // establish drain policy, reject queued ordinary work, and admit only the
    // reserved shutdown request.
    this.#releaseDispatchedRequest(id, pending);
    pending.reject(error);
    throw error;
  }

  #deferActivationEvent(notification: ActivationNotification): boolean {
    if (this.#dispatchedId === null) return false;
    const pending = this.#pending.get(this.#dispatchedId);
    if (pending?.method !== 'activation.configure') return false;
    const deferred = this.#deferredActivationEvents;
    const startsReplacementStream = deferred === null && notification.params.phase !== 'up';
    if (!startsReplacementStream && deferred?.requestId !== this.#dispatchedId) return false;
    const events = deferred?.notifications ?? [];
    if (events.length >= MAX_DEFERRED_ACTIVATION_EVENTS) {
      throw new Error('Deferred helper activation capacity exceeded');
    }
    if (deferred === null) {
      this.#deferredActivationEvents = {
        requestId: this.#dispatchedId,
        notifications: [notification],
      };
    } else {
      events.push(notification);
    }
    return true;
  }

  #validateActivationEvent(notification: ActivationNotification): void {
    const params = notification.params;
    if (params.phase === 'up') {
      const active = this.#activeActivation;
      if (active === null || !activationParamsMatch(active, params)) {
        throw new Error('Unpaired helper activation event');
      }
      this.#activeActivation = null;
      return;
    }

    if (
      params.activationGeneration <= this.#lastActivationGeneration ||
      this.#activeActivation !== null
    ) {
      throw new Error('Non-monotonic helper activation generation');
    }
    this.#lastActivationGeneration = params.activationGeneration;
    if (params.phase === 'down') this.#activeActivation = params;
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

function activationAcknowledgementMatches(
  requested: HelperParams<'activation.configure'>,
  effective: HelperResult<'activation.configure'>,
): boolean {
  return (
    effective.enabled === requested.enabled &&
    effective.bindings.length === requested.bindings.length &&
    effective.bindings.every((binding, index) => {
      const candidate = requested.bindings[index];
      return (
        binding.profileId === candidate?.profileId &&
        shortcutsEqual(binding.shortcut, candidate.shortcut)
      );
    })
  );
}

function activationParamsMatch(
  left: PairedActivationParams,
  right: PairedActivationParams,
): boolean {
  return (
    left.activationGeneration === right.activationGeneration &&
    left.targetToken === right.targetToken &&
    left.profileId === right.profileId &&
    shortcutsEqual(left.shortcut, right.shortcut)
  );
}
