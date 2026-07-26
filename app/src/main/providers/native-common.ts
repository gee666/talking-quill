import { createHash } from 'node:crypto';
import {
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
  ProviderConfigSchema,
  ProviderValidationResultSchema,
  type ModelInfo,
  type ProviderCompletionRequest,
  type ProviderConfig,
  type ProviderValidationResult,
  type VisionCapability,
} from '../../shared/schemas/providers';
import type { ProviderInvocationConfig } from './contracts';
import { ProviderError } from './errors';

export const MAX_NATIVE_MODELS = 2_000;
export const MAX_NATIVE_PAGES = 20;
export const MAX_NATIVE_OUTPUT_CHARACTERS = 200_000;

export function parseConfig(
  input: unknown,
  providerId: ProviderConfig['providerId'],
): ProviderConfig {
  try {
    const config = ProviderConfigSchema.parse(input);
    if (config.providerId !== providerId) throw new ProviderError('INVALID_CONFIG');
    return config;
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('INVALID_CONFIG');
  }
}

export function parseRequest(
  input: unknown,
): Required<Pick<ProviderCompletionRequest, 'input' | 'temperature'>> & ProviderCompletionRequest {
  try {
    return ProviderCompletionRequestSchema.parse(input);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

export function requireCredential(invocation: ProviderInvocationConfig): string {
  const credential = invocation.credential;
  if (credential === null) throw new ProviderError('MISSING_CREDENTIAL');
  if (
    credential.length < 8 ||
    credential.length > 16_384 ||
    credential.trim() !== credential ||
    hasControlCharacters(credential)
  )
    throw new ProviderError('INVALID_CONFIG');
  return credential;
}

export function credentialFingerprint(invocation: ProviderInvocationConfig): string {
  return createHash('sha256').update(requireCredential(invocation), 'utf8').digest('base64url');
}

export function requireModel(config: ProviderConfig, requestModel?: string): string {
  const model = requestModel ?? config.modelId;
  if (model === undefined || model === null || model.trim().length === 0 || model.length > 512) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return model;
}

export function record(input: unknown): Readonly<Record<string, unknown>> {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return input as Readonly<Record<string, unknown>>;
}

export function optionalRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}

export function boundedString(input: unknown, maximum = 512): string {
  if (
    typeof input !== 'string' ||
    input.trim().length === 0 ||
    input.length > maximum ||
    hasControlCharacters(input)
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return input.trim();
}

export function completionText(parts: readonly unknown[]): string {
  if (parts.length === 0 || parts.length > 256) throw new ProviderError('INVALID_RESPONSE');
  let output = '';
  for (const part of parts) {
    if (typeof part !== 'string' || part.length === 0) throw new ProviderError('INVALID_RESPONSE');
    output += part;
    if (output.length > MAX_NATIVE_OUTPUT_CHARACTERS) throw new ProviderError('INVALID_RESPONSE');
  }
  if (output.trim().length === 0) throw new ProviderError('INVALID_RESPONSE');
  return output;
}

export function modelInfo(
  id: string,
  name: string,
  contextWindow: number | null,
  vision: VisionCapability,
): ModelInfo {
  return ModelInfoSchema.parse({ id, name, contextWindow, vision });
}

export function freezeModels(models: readonly ModelInfo[]): readonly ModelInfo[] {
  const seen = new Set<string>();
  const unique: ModelInfo[] = [];
  for (const model of models) {
    if (seen.has(model.id)) continue;
    seen.add(model.id);
    unique.push(ModelInfoSchema.parse(model));
    if (unique.length > MAX_NATIVE_MODELS) throw new ProviderError('INVALID_RESPONSE');
  }
  if (unique.length === 0) throw new ProviderError('NO_MODELS');
  return Object.freeze(unique);
}

export function validation(
  destination: 'local' | 'lan' | 'cloud',
  modelCount: number,
): ProviderValidationResult {
  return ProviderValidationResultSchema.parse({ ok: true, destination, modelCount });
}

export function staticVision(modelId: string, patterns: readonly RegExp[]): VisionCapability {
  return patterns.some((pattern) => pattern.test(modelId)) ? 'supported' : 'unknown';
}

function hasControlCharacters(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

export function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}
