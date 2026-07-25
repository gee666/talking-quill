import { request as httpRequest, type IncomingMessage, type RequestOptions } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { isIP } from 'node:net';
import type { LookupFunction } from 'node:net';
import type { Destination } from '../../shared/schemas/providers';
import {
  assertValidatedEndpointPolicy,
  parseProviderUrl,
  systemEndpointResolver,
  validateProviderEndpoint,
  type EndpointResolver,
  type ValidatedEndpoint,
} from '../security/provider-endpoint-policy';
import type { EgressCategory, EgressObserver } from '../security/egress-audit';
import { ProviderError, providerErrorFromStatus, toProviderError } from './errors';

const DEFAULT_TIMEOUT_MS = 30_000;
const DEFAULT_MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
export const MAX_OPERATION_RESPONSE_BYTES = 16 * 1024 * 1024;
export const MAX_OPERATION_REQUESTS = 96;
export const MAX_PROVIDER_REQUEST_BYTES = 512 * 1024;
const MAX_REDIRECTS = 3;
const MAX_OPERATION_REDIRECTS = 24;
const MAX_OPERATION_ORIGINS = 8;
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

export type ProviderRequestKind = 'model-list' | 'model-detail' | 'completion';

export interface JsonTransportRequest {
  readonly url: string | URL;
  readonly method: 'GET' | 'POST';
  readonly kind?: ProviderRequestKind;
  readonly headers?: Readonly<Record<string, string>>;
  readonly body?: unknown;
  readonly credentialed: boolean;
  readonly fixedCloud?: boolean;
  /** Optional exact HTTPS origins allowed for this request and every redirect. */
  readonly allowedOrigins?: readonly string[];
  readonly signal: AbortSignal;
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
  readonly errorResponsePolicy?: 'gemini-api-key';
  /** Primarily useful to tighten an operation budget in deterministic tests. */
  readonly maxOperationResponseBytes?: number;
}

export interface JsonTransportResponse {
  readonly body: unknown;
  /** Destination of the final endpoint after all validated redirects. */
  readonly destination: Destination;
  readonly status: number;
}

export interface JsonTransport {
  request(options: JsonTransportRequest): Promise<JsonTransportResponse>;
  classify(
    url: string | URL,
    options: { readonly credentialed: boolean; readonly fixedCloud?: boolean },
    signal: AbortSignal,
  ): Promise<Destination>;
}

interface PreparedRequest {
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
}

interface OperationEndpoint {
  readonly endpoint: ValidatedEndpoint;
  selectedAddress: number;
}

interface OperationNetworkState {
  readonly deadline: number;
  readonly endpoints: Map<string, Promise<OperationEndpoint>>;
  readonly maxResponseBytes: number;
  responseBytes: number;
  requests: number;
  redirects: number;
}

export interface PinnedJsonTransportOptions {
  readonly category?: Extract<EgressCategory, 'provider' | 'update'>;
  readonly observeEgress?: EgressObserver;
}

export class PinnedJsonTransport implements JsonTransport {
  readonly #resolver: EndpointResolver;
  readonly #category: Extract<EgressCategory, 'provider' | 'update'>;
  readonly #observeEgress: EgressObserver;
  readonly #operations = new WeakMap<AbortSignal, OperationNetworkState>();

  constructor(
    resolver: EndpointResolver = systemEndpointResolver,
    options: PinnedJsonTransportOptions = {},
  ) {
    this.#resolver = resolver;
    this.#category = options.category ?? 'provider';
    this.#observeEgress = options.observeEgress ?? (() => undefined);
  }

  async request(options: JsonTransportRequest): Promise<JsonTransportResponse> {
    if (options.signal.aborted) throw new ProviderError('CANCELLED');
    this.#observeEgress(this.#category);
    const timeoutMs = validateBound(options.timeoutMs ?? DEFAULT_TIMEOUT_MS, 1, 120_000);
    const maxResponseBytes = validateBound(
      options.maxResponseBytes ?? DEFAULT_MAX_RESPONSE_BYTES,
      1,
      16 * 1024 * 1024,
    );
    const maxOperationResponseBytes = validateBound(
      options.maxOperationResponseBytes ?? MAX_OPERATION_RESPONSE_BYTES,
      1,
      MAX_OPERATION_RESPONSE_BYTES,
    );
    const state = this.#operationState(options.signal, timeoutMs, maxOperationResponseBytes);
    const headers = normalizeHeaders(options.headers ?? {});
    const credentialed = options.credentialed || hasCredentialHeader(headers);
    const body = serializeBody(options.body);
    if (options.method === 'GET' && body !== null) throw new ProviderError('INVALID_CONFIG');

    const timeoutController = new AbortController();
    const remaining = state.deadline - Date.now();
    if (remaining <= 0) throw new ProviderError('TIMEOUT');
    const timer = setTimeout(() => timeoutController.abort(), remaining);
    const signal = AbortSignal.any([options.signal, timeoutController.signal]);
    const prepared: PreparedRequest = {
      method: options.method,
      kind: options.kind ?? 'completion',
      headers,
      body,
      credentialed,
      signal,
      maxResponseBytes,
      ...(options.fixedCloud === undefined ? {} : { fixedCloud: options.fixedCloud }),
      ...(options.allowedOrigins === undefined
        ? {}
        : { allowedOrigins: normalizeAllowedOrigins(options.allowedOrigins) }),
      ...(options.errorResponsePolicy === undefined
        ? {}
        : { errorResponsePolicy: options.errorResponsePolicy }),
    };
    try {
      return await this.#requestFollowingRedirects(options.url, prepared, state, null, 0);
    } catch (error: unknown) {
      if (isAborted(timeoutController.signal) && !isAborted(options.signal)) {
        throw new ProviderError('TIMEOUT');
      }
      if (isAborted(options.signal)) throw new ProviderError('CANCELLED');
      throw toProviderError(error);
    } finally {
      clearTimeout(timer);
    }
  }

  async classify(
    url: string | URL,
    options: { readonly credentialed: boolean; readonly fixedCloud?: boolean },
    signal: AbortSignal,
  ): Promise<Destination> {
    if (signal.aborted) throw new ProviderError('CANCELLED');
    const state = this.#operationState(signal, DEFAULT_TIMEOUT_MS, MAX_OPERATION_RESPONSE_BYTES);
    const timeoutController = new AbortController();
    const remaining = state.deadline - Date.now();
    if (remaining <= 0) throw new ProviderError('TIMEOUT');
    const timer = setTimeout(() => timeoutController.abort(), remaining);
    const operationSignal = AbortSignal.any([signal, timeoutController.signal]);
    try {
      return (
        await this.#endpointFor(url, state, {
          credentialed: options.credentialed,
          signal: operationSignal,
          ...(options.fixedCloud === undefined ? {} : { fixedCloud: options.fixedCloud }),
        })
      ).endpoint.destination;
    } catch (error: unknown) {
      if (isAborted(timeoutController.signal) && !isAborted(signal)) {
        throw new ProviderError('TIMEOUT');
      }
      if (isAborted(signal)) throw new ProviderError('CANCELLED');
      throw toProviderError(error);
    } finally {
      clearTimeout(timer);
    }
  }

  #operationState(
    signal: AbortSignal,
    timeoutMs: number,
    maxResponseBytes: number,
  ): OperationNetworkState {
    const existing = this.#operations.get(signal);
    if (existing !== undefined) return existing;
    const state: OperationNetworkState = {
      deadline: Date.now() + timeoutMs,
      endpoints: new Map(),
      maxResponseBytes,
      responseBytes: 0,
      requests: 0,
      redirects: 0,
    };
    this.#operations.set(signal, state);
    return state;
  }

  async #requestFollowingRedirects(
    url: string | URL,
    options: PreparedRequest,
    state: OperationNetworkState,
    requiredDestination: Destination | null,
    redirects: number,
  ): Promise<JsonTransportResponse> {
    throwIfAborted(options.signal);
    const pinned = await this.#endpointFor(url, state, {
      credentialed: options.credentialed,
      signal: options.signal,
      ...(options.fixedCloud === undefined ? {} : { fixedCloud: options.fixedCloud }),
    });
    const endpoint = endpointAtSelectedAddress(pinned, parseProviderUrl(url));
    if (options.allowedOrigins !== undefined && !options.allowedOrigins.has(endpoint.url.origin)) {
      throw new ProviderError('SECURITY_BLOCKED');
    }
    if (requiredDestination !== null && endpoint.destination !== requiredDestination) {
      throw new ProviderError('SECURITY_BLOCKED');
    }

    const response = await this.#performWithSafeAddressRetry(pinned, endpoint, options, state);
    if (!isRedirect(response.status)) return response;
    state.redirects += 1;
    if (
      redirects >= MAX_REDIRECTS ||
      state.redirects > MAX_OPERATION_REDIRECTS ||
      response.location === null
    ) {
      throw new ProviderError('REMOTE_FAILURE');
    }

    let next: URL;
    try {
      next = parseProviderUrl(new URL(response.location, endpoint.url));
    } catch (error: unknown) {
      if (error instanceof ProviderError) throw error;
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (endpoint.url.protocol === 'https:' && next.protocol !== 'https:') {
      throw new ProviderError('SECURITY_BLOCKED');
    }
    const sameOrigin = next.origin === endpoint.url.origin;
    if (!sameOrigin && (options.credentialed || options.body !== null)) {
      // Never replay authorization or transcript content to another origin. Public GET model
      // discovery may follow a canonical-host redirect only after the new origin is validated.
      throw new ProviderError('SECURITY_BLOCKED');
    }

    return this.#requestFollowingRedirects(
      next,
      redirectRequest(options, response.status),
      state,
      sameOrigin ? (requiredDestination ?? endpoint.destination) : requiredDestination,
      redirects + 1,
    );
  }

  async #endpointFor(
    value: string | URL,
    state: OperationNetworkState,
    options: {
      readonly credentialed: boolean;
      readonly fixedCloud?: boolean;
      readonly signal: AbortSignal;
    },
  ): Promise<OperationEndpoint> {
    const url = parseProviderUrl(value);
    const cached = state.endpoints.get(url.origin);
    if (cached !== undefined) {
      const result = await cached;
      assertValidatedEndpointPolicy(result.endpoint, url, options);
      return result;
    }
    if (state.endpoints.size >= MAX_OPERATION_ORIGINS) {
      throw new ProviderError('SECURITY_BLOCKED');
    }
    const pending = validateProviderEndpoint(url, this.#resolver, options).then(
      (endpoint): OperationEndpoint => ({ endpoint, selectedAddress: 0 }),
    );
    state.endpoints.set(url.origin, pending);
    try {
      return await pending;
    } catch (error: unknown) {
      if (state.endpoints.get(url.origin) === pending) state.endpoints.delete(url.origin);
      throw error;
    }
  }

  async #performWithSafeAddressRetry(
    pinned: OperationEndpoint,
    endpoint: ValidatedEndpoint,
    options: PreparedRequest,
    state: OperationNetworkState,
  ): Promise<RawResponse> {
    const attempts =
      options.method === 'GET' && options.body === null ? endpoint.addresses.length : 1;
    let lastError: unknown = new ProviderError('UNAVAILABLE');
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      const addressIndex = (pinned.selectedAddress + attempt) % endpoint.addresses.length;
      const selected = endpoint.addresses[addressIndex];
      if (selected === undefined) throw new ProviderError('SECURITY_BLOCKED');
      const candidate = Object.freeze({ ...endpoint, pinnedAddress: selected });
      try {
        this.#consumeRequest(state);
        const response = await performRequest(candidate, options, state);
        pinned.selectedAddress = addressIndex;
        return response;
      } catch (error: unknown) {
        lastError = error;
        if (!isRetryableSocketFailure(error) || attempt + 1 >= attempts) throw error;
      }
    }
    throw lastError;
  }

  #consumeRequest(state: OperationNetworkState): void {
    state.requests += 1;
    if (state.requests > MAX_OPERATION_REQUESTS) throw new ProviderError('REMOTE_FAILURE');
  }
}

interface RawResponse extends JsonTransportResponse {
  readonly location: string | null;
}

function performRequest(
  endpoint: ValidatedEndpoint,
  options: PreparedRequest,
  state: OperationNetworkState,
): Promise<RawResponse> {
  return new Promise((resolve, reject) => {
    if (options.signal.aborted) {
      reject(new ProviderError('CANCELLED'));
      return;
    }
    const lookup: LookupFunction = (_hostname, lookupOptions, callback) => {
      if (lookupOptions.all === true) {
        callback(null, [
          {
            address: endpoint.pinnedAddress.address,
            family: endpoint.pinnedAddress.family,
          },
        ]);
        return;
      }
      callback(null, endpoint.pinnedAddress.address, endpoint.pinnedAddress.family);
    };
    const requestOptions: RequestOptions = {
      method: options.method,
      protocol: endpoint.url.protocol,
      hostname: endpoint.url.hostname,
      port: endpoint.url.port,
      path: `${endpoint.url.pathname}${endpoint.url.search}`,
      family: endpoint.pinnedAddress.family,
      lookup,
      agent: false,
      headers: {
        accept: 'application/json',
        'accept-encoding': 'identity',
        ...(options.body === null ? {} : { 'content-type': 'application/json' }),
        ...options.headers,
      },
      ...(endpoint.url.protocol === 'https:' && isIP(stripBrackets(endpoint.url.hostname)) === 0
        ? { servername: stripBrackets(endpoint.url.hostname) }
        : {}),
    };
    const makeRequest = endpoint.url.protocol === 'https:' ? httpsRequest : httpRequest;
    const request = makeRequest(requestOptions, (response) => {
      const status = response.statusCode ?? 0;
      const location =
        typeof response.headers.location === 'string' ? response.headers.location : null;
      if (isRedirect(status)) {
        response.destroy();
        resolve({ body: null, destination: endpoint.destination, status, location });
        return;
      }
      if (status < 200 || status >= 300) {
        if (status === 400 && options.errorResponsePolicy === 'gemini-api-key') {
          consumeGeminiErrorResponse(response, state).then(reject, reject);
        } else {
          response.destroy();
          reject(providerErrorFromStatus(status, options.kind));
        }
        return;
      }
      const declaredLength = Number(response.headers['content-length']);
      if (
        Number.isFinite(declaredLength) &&
        (declaredLength > options.maxResponseBytes ||
          state.responseBytes + declaredLength > state.maxResponseBytes)
      ) {
        response.destroy();
        reject(new ProviderError('RESPONSE_TOO_LARGE'));
        return;
      }
      const contentType = response.headers['content-type'];
      if (
        typeof contentType !== 'string' ||
        !/^application\/(?:[a-z0-9.+-]*\+)?json\b/i.test(contentType)
      ) {
        response.destroy();
        reject(new ProviderError('INVALID_RESPONSE'));
        return;
      }
      const chunks: Buffer[] = [];
      let size = 0;
      response.on('data', (chunk: Buffer | string) => {
        const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        size += bytes.length;
        state.responseBytes += bytes.length;
        if (size > options.maxResponseBytes || state.responseBytes > state.maxResponseBytes) {
          response.destroy(new ProviderError('RESPONSE_TOO_LARGE'));
          return;
        }
        chunks.push(bytes);
      });
      response.on('error', reject);
      response.on('end', () => {
        try {
          const source = Buffer.concat(chunks, size).toString('utf8');
          const body = JSON.parse(source) as unknown;
          resolve({ body, destination: endpoint.destination, status, location: null });
        } catch {
          reject(new ProviderError('INVALID_RESPONSE'));
        }
      });
    });
    request.on('error', reject);
    const abort = (): void => {
      request.destroy(new ProviderError('CANCELLED'));
    };
    options.signal.addEventListener('abort', abort, { once: true });
    request.on('close', () => options.signal.removeEventListener('abort', abort));
    if (options.body !== null) request.write(options.body);
    request.end();
  });
}

async function consumeGeminiErrorResponse(
  response: IncomingMessage,
  state: OperationNetworkState,
): Promise<ProviderError> {
  const maximumBytes = 16 * 1_024;
  const declaredLength = Number(response.headers['content-length']);
  if (Number.isFinite(declaredLength) && declaredLength > maximumBytes) {
    response.destroy();
    return new ProviderError('REMOTE_FAILURE');
  }
  const chunks: Buffer[] = [];
  let size = 0;
  try {
    for await (const chunk of response) {
      const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as Uint8Array);
      size += bytes.length;
      state.responseBytes += bytes.length;
      if (size > maximumBytes || state.responseBytes > state.maxResponseBytes) {
        response.destroy();
        return new ProviderError('REMOTE_FAILURE');
      }
      chunks.push(bytes);
    }
    const parsed = JSON.parse(Buffer.concat(chunks, size).toString('utf8')) as unknown;
    return isGeminiInvalidApiKey(parsed)
      ? new ProviderError('AUTHENTICATION_FAILED')
      : new ProviderError('REMOTE_FAILURE');
  } catch {
    return new ProviderError('REMOTE_FAILURE');
  }
}

function isGeminiInvalidApiKey(input: unknown): boolean {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return false;
  const error = 'error' in input ? input.error : null;
  if (typeof error !== 'object' || error === null || Array.isArray(error)) return false;
  if (!('status' in error) || error.status !== 'INVALID_ARGUMENT') return false;
  if (!('details' in error) || !Array.isArray(error.details) || error.details.length > 32) {
    return false;
  }
  return error.details.some((detail: unknown) => {
    if (typeof detail !== 'object' || detail === null || Array.isArray(detail)) return false;
    const fields = detail as Readonly<Record<string, unknown>>;
    return fields.reason === 'API_KEY_INVALID' && fields.domain === 'googleapis.com';
  });
}

function endpointAtSelectedAddress(pinned: OperationEndpoint, url: URL): ValidatedEndpoint {
  const address = pinned.endpoint.addresses[pinned.selectedAddress];
  if (address === undefined) throw new ProviderError('SECURITY_BLOCKED');
  return Object.freeze({ ...pinned.endpoint, url, pinnedAddress: address });
}

function redirectRequest(options: PreparedRequest, status: number): PreparedRequest {
  if (status === 307 || status === 308 || options.method === 'GET') return options;
  if (status === 303) return Object.freeze({ ...options, method: 'GET', body: null });
  // Replaying transcript POST data after ambiguous 301/302 responses is deliberately forbidden.
  throw new ProviderError('SECURITY_BLOCKED');
}

function serializeBody(body: unknown): Buffer | null {
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

function normalizeHeaders(
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

function normalizeAllowedOrigins(origins: readonly string[]): ReadonlySet<string> {
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

function hasCredentialHeader(headers: Readonly<Record<string, string>>): boolean {
  return Object.keys(headers).some(
    (name) =>
      CREDENTIAL_HEADERS.has(name) || /(?:api[-_]?key|auth|credential|secret|token)/i.test(name),
  );
}

function isRedirect(status: number): boolean {
  return status === 301 || status === 302 || status === 303 || status === 307 || status === 308;
}

function validateBound(value: number, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}

function stripBrackets(hostname: string): string {
  return hostname.startsWith('[') && hostname.endsWith(']') ? hostname.slice(1, -1) : hostname;
}

function throwIfAborted(signal: AbortSignal): void {
  if (isAborted(signal)) throw new ProviderError('CANCELLED');
}

function isAborted(signal: AbortSignal): boolean {
  return signal.aborted;
}

function isRetryableSocketFailure(error: unknown): boolean {
  return !(error instanceof ProviderError);
}
