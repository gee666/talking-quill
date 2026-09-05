import type { ValidatedEndpoint } from '../security/provider-endpoint-policy';
import type { ProviderRequestKind } from './json-transport';
import { ProviderError } from './errors';

export const MAX_PROVIDER_REQUEST_BYTES = 512 * 1024;

const FORBIDDEN_HEADERS = new Set([
  'connection',
  'content-length',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
  'forwarded',
  'x-forwarded-for',
  'x-forwarded-host',
  'x-forwarded-proto',
]);
const CREDENTIAL_HEADERS = new Set([
  'authorization',
  'cookie',
  'x-api-key',
  'api-key',
  'x-goog-api-key',
  'ocp-apim-subscription-key',
]);

export interface PreparedRequest {
  readonly method: 'GET' | 'POST';
  readonly kind: ProviderRequestKind;
  readonly headers: Readonly<Record<string, string>>;
  readonly body: Buffer | null;
  readonly credentialed: boolean;
  readonly fixedCloud?: boolean;
  readonly allowedOrigins?: ReadonlySet<string>;
  readonly signal: AbortSignal;
  readonly maxResponseBytes: number;
  readonly errorResponsePolicy?: 'gemini-api-key';
  readonly responseType: 'json' | 'bytes';
}

export interface OperationEndpoint {
  readonly endpoint: ValidatedEndpoint;
  selectedAddress: number;
}

export interface OperationNetworkState {
  readonly deadline: number;
  readonly endpoints: Map<string, Promise<OperationEndpoint>>;
  readonly maxResponseBytes: number;
  responseBytes: number;
  requests: number;
  redirects: number;
}

export function endpointAtSelectedAddress(pinned: OperationEndpoint, url: URL): ValidatedEndpoint {
  const address = pinned.endpoint.addresses[pinned.selectedAddress];
  if (address === undefined) throw new ProviderError('SECURITY_BLOCKED');
  return Object.freeze({ ...pinned.endpoint, url, pinnedAddress: address });
}

export function redirectRequest(options: PreparedRequest, status: number): PreparedRequest {
  if (status === 307 || status === 308 || options.method === 'GET') return options;
  if (status === 303) return Object.freeze({ ...options, method: 'GET', body: null });
  // Replaying transcript POST data after ambiguous 301/302 responses is deliberately forbidden.
  throw new ProviderError('SECURITY_BLOCKED');
}

export function serializeBody(body: unknown): Buffer | null {
  if (body === undefined) return null;
  let source: string;
  try {
    const encoded: unknown = JSON.stringify(body);
    if (typeof encoded !== 'string') throw new ProviderError('INVALID_CONFIG');
    source = encoded;
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('INVALID_CONFIG');
  }
  const bytes = Buffer.from(source, 'utf8');
  if (bytes.length > MAX_PROVIDER_REQUEST_BYTES) throw new ProviderError('REQUEST_TOO_LARGE');
  return bytes;
}

export function normalizeHeaders(
  headers: Readonly<Record<string, string>>,
): Readonly<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const [rawName, rawValue] of Object.entries(headers)) {
    const name = rawName.toLowerCase();
    if (!/^[a-z0-9!#$%&'*+.^_`|~-]+$/.test(name) || FORBIDDEN_HEADERS.has(name)) {
      throw new ProviderError('INVALID_CONFIG');
    }
    if (/\r|\n/.test(rawValue)) throw new ProviderError('INVALID_CONFIG');
    result[name] = rawValue;
  }
  return Object.freeze(result);
}

export function normalizeAllowedOrigins(origins: readonly string[]): ReadonlySet<string> {
  if (origins.length === 0 || origins.length > 8) throw new ProviderError('INVALID_CONFIG');
  const normalized = new Set<string>();
  for (const value of origins) {
    const url = new URL(value);
    if (url.protocol !== 'https:' || url.origin !== value || url.username || url.password) {
      throw new ProviderError('INVALID_CONFIG');
    }
    normalized.add(url.origin);
  }
  return normalized;
}

export function hasCredentialHeader(headers: Readonly<Record<string, string>>): boolean {
  return Object.keys(headers).some(
    (name) =>
      CREDENTIAL_HEADERS.has(name) || /(?:api[-_]?key|auth|credential|secret|token)/i.test(name),
  );
}

export function isRedirect(status: number): boolean {
  return status === 301 || status === 302 || status === 303 || status === 307 || status === 308;
}

export function validateBound(value: number, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}
