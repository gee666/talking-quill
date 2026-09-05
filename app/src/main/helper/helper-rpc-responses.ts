import { type HelperRpcRuntime } from './helper-rpc-runtime';
import { type PendingRequest } from './helper-rpc-channel';

export function releaseDispatchedRequest(
  this: HelperRpcRuntime,
  id: number,
  pending: PendingRequest,
): void {
  if (pending.timer !== null) clearTimeout(pending.timer);
  pending.removeAbort();
  this.pending.delete(id);
  if (this.dispatchedId === id) this.dispatchedId = null;
}

export function rejectMalformedDispatchedResponse(
  this: HelperRpcRuntime,
  id: number,
  pending: PendingRequest,
  message: string,
): never {
  const error = this.options.createError('transport-error', message);
  // Release only the malformed request. Do not pump here: onFault must first
  // establish drain policy, reject queued ordinary work, and admit only the
  // reserved shutdown request.
  this.releaseDispatchedRequest(id, pending);
  pending.reject(error);
  throw error;
}
