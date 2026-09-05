import { performance } from 'node:perf_hooks';
import {
  helperParamsSchemas,
  helperResultSchemas,
  HelperNotificationSchema,
  HelperRpcResponseSchema,
  type HelperMethod,
  type HelperParams,
  type HelperResult,
} from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { decodeHelperJson, encodeHelperFrame, HelperFrameDecoder } from './framing';
import { activationAcknowledgementMatches } from './helper-rpc-activation';
import {
  HelperRpcRuntime,
  type HelperRpcStreams,
  type HelperRpcSession,
  type HelperRpcChannelOptions,
  type HelperRpcRequestOptions,
} from './helper-rpc-runtime';

export type { HelperRpcErrorCode, HelperRpcSession } from './helper-rpc-runtime';

export interface PendingRequest {
  readonly method: HelperMethod;
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

const MAX_ORDINARY_REQUESTS = 256;
const RESERVED_SHUTDOWN_REQUESTS = 1;

/** Internal transport for the one helper process currently owned by HelperClient. */
export class HelperRpcChannel {
  readonly #runtime: HelperRpcRuntime;

  constructor(options: HelperRpcChannelOptions) {
    this.#runtime = new HelperRpcRuntime(
      options,
      (session) => this.isCurrent(session),
      (session, payload) => this.#acceptPayload(session, payload),
    );
  }

  attach(child: HelperRpcStreams): HelperRpcSession {
    if (this.#runtime.session !== null) {
      throw this.#runtime.options.createError(
        'transport-error',
        'Native helper RPC channel is already attached',
      );
    }
    const session = Object.freeze({ child, token: Symbol('helper-rpc-session') });
    this.#runtime.session = session;
    this.#runtime.decoder = new HelperFrameDecoder();
    this.#runtime.ignoredResponseIds.clear();
    this.#runtime.writeQueue.length = 0;
    this.#runtime.writeBlocked = false;
    this.#runtime.writeClosed = false;
    this.#runtime.dispatchedId = null;
    this.#runtime.draining = false;
    this.#runtime.lastActivationGeneration = 0;
    this.#runtime.activeActivation = null;
    this.#runtime.deferredActivationEvents = null;
    this.#runtime.attachStreams(session);
    return session;
  }

  isCurrent(session: HelperRpcSession): boolean {
    return this.#runtime.session === session && !this.#runtime.writeClosed;
  }

  resetOwnerActivationStream(session: HelperRpcSession): void {
    if (this.#runtime.session !== session) return;
    this.#runtime.lastActivationGeneration = 0;
    this.#runtime.activeActivation = null;
    this.#runtime.deferredActivationEvents = null;
  }

  beginDraining(session: HelperRpcSession): void {
    if (this.#runtime.session !== session) return;
    if (!this.#runtime.draining) {
      this.#runtime.draining = true;
      const error = this.#runtime.options.createError(
        'not-running',
        'Native helper is terminating',
      );
      for (let index = this.#runtime.writeQueue.length - 1; index >= 0; index -= 1) {
        const queued = this.#runtime.writeQueue[index];
        if (queued === undefined || queued.allowDraining) continue;
        this.#runtime.writeQueue.splice(index, 1);
        this.#runtime.rejectQueuedRequest(queued.id, error);
      }
    }
    // A correlated protocol fault releases dispatched authority without
    // pumping. The fault supervisor calls this method again only after it has
    // rejected ordinary work and deliberately admitted reserved shutdown.
    this.#runtime.pumpWrites(session);
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
      this.#runtime.session !== session ||
      this.#runtime.writeClosed ||
      (this.#runtime.draining && !options.allowDraining) ||
      child.stdin.destroyed ||
      !child.stdin.writable
    ) {
      return Promise.reject(
        this.#runtime.options.createError('not-running', 'Native helper is terminating'),
      );
    }

    const requestCapacity = options.allowDraining
      ? MAX_ORDINARY_REQUESTS + RESERVED_SHUTDOWN_REQUESTS
      : MAX_ORDINARY_REQUESTS;
    if (this.#runtime.pending.size >= requestCapacity) {
      return Promise.reject(
        this.#runtime.options.createError(
          'request-capacity',
          'Native helper request capacity is full',
        ),
      );
    }

    const id = this.#runtime.takeRequestId();
    const validParams = helperParamsSchemas[method].parse(params);
    const frame = encodeHelperFrame({ jsonrpc: '2.0', id, method, params: validParams });
    return new Promise<HelperResult<Method>>((resolve, reject) => {
      const abort = (): void => {
        if (this.#runtime.session !== session) return;
        const pending = this.#runtime.pending.get(id);
        if (pending === undefined) return;
        if (pending.dispatched) {
          pending.abortRequested = true;
          return;
        }
        const queuedIndex = this.#runtime.writeQueue.findIndex((queued) => queued.id === id);
        if (queuedIndex !== -1) this.#runtime.writeQueue.splice(queuedIndex, 1);
        if (pending.timer !== null) clearTimeout(pending.timer);
        pending.removeAbort();
        this.#runtime.pending.delete(id);
        pending.reject(new DOMException('Native helper request cancelled', 'AbortError'));
      };
      options.signal?.addEventListener('abort', abort, { once: true });
      this.#runtime.pending.set(id, {
        method,
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
        resolve: (result) => resolve(result as HelperResult<Method>),
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
      this.#runtime.armRequestTimeout(session, id);
      const queued = { id, frame, allowDraining: options.allowDraining };
      if (options.allowDraining || options.priority === true)
        this.#runtime.writeQueue.unshift(queued);
      else this.#runtime.writeQueue.push(queued);
      this.#runtime.pumpWrites(session);
    });
  }

  close(session: HelperRpcSession, pendingError: Error): void {
    if (this.#runtime.session !== session) return;
    this.#runtime.writeClosed = true;
    this.#runtime.draining = true;
    this.#runtime.writeBlocked = false;
    this.#runtime.dispatchedId = null;
    this.#runtime.writeQueue.length = 0;
    this.#runtime.ignoredResponseIds.clear();
    this.#runtime.lastActivationGeneration = 0;
    this.#runtime.activeActivation = null;
    this.#runtime.deferredActivationEvents = null;
    this.#runtime.rejectPending(pendingError);
    this.#runtime.session = null;
  }

  #acceptPayload(session: HelperRpcSession, payload: Buffer): void {
    const runtime = this.#runtime;
    if (!this.isCurrent(session)) return;
    const raw = decodeHelperJson(payload);
    const response = HelperRpcResponseSchema.safeParse(raw);
    if (response.success) {
      if (typeof response.data.id !== 'number') {
        throw new Error('Uncorrelated helper response');
      }
      if (runtime.ignoredResponseIds.delete(response.data.id)) return;
      const pending = runtime.pending.get(response.data.id);
      if (
        pending === undefined ||
        !pending.dispatched ||
        runtime.dispatchedId !== response.data.id
      ) {
        throw new Error('Helper response does not match dispatched authority');
      }
      if ('error' in response.data) {
        if (runtime.deferredActivationEvents?.requestId === response.data.id) {
          runtime.deferredActivationEvents = null;
        }
        runtime.releaseDispatchedRequest(response.data.id, pending);
        pending.reject(
          runtime.options.createError(
            'rpc-error',
            `Native helper rejected ${pending.method}`,
            response.data.error.code,
          ),
        );
        runtime.pumpWrites(session);
        return;
      }
      const result = helperResultSchemas[pending.method].safeParse(response.data.result);
      if (!result.success) {
        runtime.rejectMalformedDispatchedResponse(
          response.data.id,
          pending,
          'Invalid helper result schema',
        );
      }
      if (pending.method === 'paste.inject') {
        const pasteSubmitted = helperResultSchemas['paste.inject'].parse(result.data).submitted;
        if (pasteSubmitted !== pending.pasteCommitted) {
          runtime.rejectMalformedDispatchedResponse(
            response.data.id,
            pending,
            pasteSubmitted
              ? 'Native helper acknowledged paste before its commit notification'
              : 'Native helper committed a paste before reporting rejection',
          );
        }
      }
      if (pending.method === 'ping') {
        runtime.options.onPingResult?.(session, helperResultSchemas.ping.parse(result.data));
      }
      if (pending.method === 'activation.configure') {
        const requested = pending.activationConfiguration;
        const effective = helperResultSchemas['activation.configure'].parse(result.data);
        const deferred = runtime.deferredActivationEvents;
        if (deferred?.requestId === response.data.id) runtime.deferredActivationEvents = null;
        if (requested !== null && activationAcknowledgementMatches(requested, effective)) {
          // Configuration replacement cancels the old pair. Publish events held
          // behind that response only after validating the acknowledgement.
          runtime.activeActivation = null;
          for (const event of deferred?.notifications ?? []) {
            runtime.validateActivationEvent(event);
            runtime.options.onNotification(session, event);
          }
        }
      }
      // Only a validated result may release the slot and expose its successor.
      runtime.releaseDispatchedRequest(response.data.id, pending);
      pending.resolve(result.data);
      runtime.pumpWrites(session);
      return;
    }

    const notification = HelperNotificationSchema.parse(raw);
    if (notification.method === 'activation.event') {
      if (runtime.deferActivationEvent(notification)) return;
      runtime.validateActivationEvent(notification);
    }
    if (notification.method === 'paste.committed') {
      if (typeof notification.params.requestId !== 'number') {
        throw new Error('Unknown paste commit request ID');
      }
      const pending = runtime.pending.get(notification.params.requestId);
      if (
        pending?.method !== 'paste.inject' ||
        !pending.dispatched ||
        runtime.dispatchedId !== notification.params.requestId
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
    runtime.options.onNotification(session, notification);
  }
}
