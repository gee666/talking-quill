import { ZodError } from 'zod';
import type { PublicProviderError, PublicProviderErrorCode } from '../../shared/schemas/providers';
import type { ProviderRequestKind } from './json-transport';

const PUBLIC_MESSAGES = Object.freeze({
  INVALID_CONFIG: 'The provider configuration is invalid.',
  STALE_CONFIG: 'The provider configuration changed. Refresh it and try again.',
  MISSING_CREDENTIAL: 'A provider credential is required.',
  SECURITY_BLOCKED: 'The provider destination was blocked by the security policy.',
  UNAVAILABLE: 'The provider is unavailable.',
  PI_NOT_FOUND: 'Pi is not installed in a supported location.',
  PI_CONFIG_INVALID: 'The configured Pi executable or directory is invalid.',
  PI_INCOMPATIBLE: 'Pi does not expose the CLI capabilities Talking Quill requires.',
  PI_LAUNCH_FAILED: 'Pi could not complete capability validation, launch, or cleanup.',
  AUTHENTICATION_FAILED: 'The provider rejected the credential.',
  RATE_LIMITED: 'The provider rate limit was reached.',
  MODEL_NOT_FOUND: 'The selected model is not available.',
  NO_MODELS: 'The provider has no available models.',
  TIMEOUT: 'The provider request timed out.',
  CANCELLED: 'The provider request was cancelled.',
  REQUEST_TOO_LARGE: 'The provider request exceeded the allowed size.',
  RESPONSE_TOO_LARGE: 'The provider response exceeded the allowed size.',
  INVALID_RESPONSE: 'The provider returned an invalid response.',
  REMOTE_FAILURE: 'The provider request failed.',
} satisfies Readonly<Record<PublicProviderErrorCode, string>>);

const RETRYABLE_CODES = new Set<PublicProviderErrorCode>([
  'UNAVAILABLE',
  'PI_NOT_FOUND',
  'PI_LAUNCH_FAILED',
  'RATE_LIMITED',
  'TIMEOUT',
  'REMOTE_FAILURE',
]);

export class ProviderError extends Error {
  readonly code: PublicProviderErrorCode;
  readonly retryable: boolean;

  constructor(code: PublicProviderErrorCode) {
    super(PUBLIC_MESSAGES[code]);
    this.name = 'ProviderError';
    this.code = code;
    this.retryable = RETRYABLE_CODES.has(code);
  }

  toPublicError(): PublicProviderError {
    return Object.freeze({
      code: this.code,
      message: PUBLIC_MESSAGES[this.code],
      retryable: this.retryable,
    });
  }
}

export function toProviderError(error: unknown): ProviderError {
  if (error instanceof ProviderError) return error;
  if (error instanceof ZodError) return new ProviderError('INVALID_CONFIG');
  if (isAbortError(error)) return new ProviderError('CANCELLED');
  return new ProviderError('UNAVAILABLE');
}

export function providerErrorFromStatus(
  status: number,
  requestKind: ProviderRequestKind = 'completion',
): ProviderError {
  if (status === 401 || status === 403) return new ProviderError('AUTHENTICATION_FAILED');
  if (status === 404) {
    return new ProviderError(requestKind === 'model-list' ? 'REMOTE_FAILURE' : 'MODEL_NOT_FOUND');
  }
  if (status === 408 || status === 504) return new ProviderError('TIMEOUT');
  if (status === 413) return new ProviderError('RESPONSE_TOO_LARGE');
  if (status === 429) return new ProviderError('RATE_LIMITED');
  if (status >= 500) return new ProviderError('UNAVAILABLE');
  return new ProviderError('REMOTE_FAILURE');
}

function isAbortError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  if (error.name === 'AbortError') return true;
  return 'code' in error && error.code === 'ABORT_ERR';
}
