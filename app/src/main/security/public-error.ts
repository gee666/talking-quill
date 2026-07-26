import { ZodError } from 'zod';
import { VaultUnavailableError } from '../persistence/credential-vault';
import { ProviderError } from '../providers/errors';
import { ModelManagerError } from '../transcription/errors';
import { PublicErrorSchema, type PublicError } from '../../shared/ipc/registry';

export class PublicAppError extends Error {
  readonly publicError: PublicError;

  constructor(error: PublicError) {
    super(error.message);
    this.name = 'PublicAppError';
    this.publicError = PublicErrorSchema.parse(error);
  }
}

export function toPublicError(error: unknown): PublicError {
  if (error instanceof PublicAppError) return error.publicError;
  if (error instanceof ZodError) {
    return { code: 'BAD_REQUEST', message: 'The request was invalid.' };
  }
  if (error instanceof VaultUnavailableError) {
    return { code: 'UNAVAILABLE', message: 'Secure credential storage is unavailable.' };
  }
  if (error instanceof ModelManagerError) {
    if (error.code === 'CANCELLED') {
      return { code: 'CANCELLED', message: 'The operation was cancelled.' };
    }
    if (
      ['OFFLINE', 'TIMEOUT', 'HTTP', 'FILE_LOCKED', 'IO', 'WORKER_VALIDATION'].includes(error.code)
    ) {
      return { code: 'UNAVAILABLE', message: error.message };
    }
    if (['DISK_SPACE', 'CORRUPT', 'BUSY', 'PROTOCOL'].includes(error.code)) {
      return { code: 'BAD_REQUEST', message: error.message };
    }
  }
  if (error instanceof ProviderError) {
    const providerError = error.toPublicError();
    return { code: providerError.code, message: providerError.message };
  }
  return { code: 'INTERNAL', message: 'The operation could not be completed.' };
}
