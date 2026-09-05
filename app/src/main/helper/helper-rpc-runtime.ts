import { type HelperNotification, type HelperResult } from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { HelperFrameDecoder } from './framing';
import {
  attachStreams,
  armRequestTimeout,
  expireRequest,
  pumpWrites,
  failTransport,
  takeRequestId,
  rejectQueuedRequest,
  rejectPending,
} from './helper-rpc-transport';
import { type PendingRequest } from './helper-rpc-channel';
import {
  releaseDispatchedRequest,
  rejectMalformedDispatchedResponse,
} from './helper-rpc-responses';
import { deferActivationEvent, validateActivationEvent } from './helper-rpc-activation';

export type HelperRpcErrorCode =
  'not-running' | 'request-capacity' | 'request-timeout' | 'rpc-error' | 'transport-error';

export interface HelperRpcStreams {
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

export type ActivationNotification = Extract<HelperNotification, { method: 'activation.event' }>;

export type PairedActivationParams = Extract<
  ActivationNotification['params'],
  { phase: 'down' | 'up' }
>;

interface QueuedWrite {
  readonly id: number;
  readonly frame: Buffer;
  readonly allowDraining: boolean;
}

export interface HelperRpcChannelOptions {
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

export interface HelperRpcRequestOptions {
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

/** Channel state across process sessions, kept private behind HelperRpcChannel.
 * Dispatch, response and activation operations share this instance without copying authority.
 * Their runtime imports are type-only; this class wires the operations together.
 */
export class HelperRpcRuntime {
  readonly options: HelperRpcChannelOptions;
  readonly pending = new Map<number, PendingRequest>();
  readonly ignoredResponseIds = new Set<number>();
  readonly writeQueue: QueuedWrite[] = [];
  session: HelperRpcSession | null = null;
  decoder = new HelperFrameDecoder();
  writeBlocked = false;
  writeClosed = true;
  dispatchedId: number | null = null;
  draining = false;
  nextRequestId = 1;
  lastActivationGeneration = 0;
  activeActivation: PairedActivationParams | null = null;
  deferredActivationEvents: {
    readonly requestId: number;
    readonly notifications: ActivationNotification[];
  } | null = null;

  constructor(
    options: HelperRpcChannelOptions,
    readonly isCurrent: (session: HelperRpcSession) => boolean,
    readonly acceptPayload: (session: HelperRpcSession, payload: Buffer) => void,
  ) {
    this.options = options;
  }

  readonly attachStreams = attachStreams;
  readonly armRequestTimeout = armRequestTimeout;
  readonly expireRequest = expireRequest;
  readonly pumpWrites = pumpWrites;
  readonly failTransport = failTransport;
  readonly takeRequestId = takeRequestId;
  readonly rejectQueuedRequest = rejectQueuedRequest;
  readonly rejectPending = rejectPending;
  readonly releaseDispatchedRequest = releaseDispatchedRequest;
  readonly rejectMalformedDispatchedResponse = rejectMalformedDispatchedResponse;
  readonly deferActivationEvent = deferActivationEvent;
  readonly validateActivationEvent = validateActivationEvent;
}
