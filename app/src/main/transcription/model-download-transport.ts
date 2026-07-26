import {
  MODEL_DOWNLOAD_MAX_REDIRECTS,
  MODEL_DOWNLOAD_REQUEST_TIMEOUT_MS,
} from '../../shared/constants/whisper';
import type { ModelManifestEntry, ModelManifestFile } from '../../shared/schemas/model-manifest';
import type { EgressObserver } from '../security/egress-audit';
import { ModelManagerError } from './errors';

interface ModelDownloadTransportOptions {
  readonly fetch?: typeof fetch;
  readonly urlFor?: (model: ModelManifestEntry, file: ModelManifestFile) => string;
  readonly validateRequestUrl?: (url: string) => boolean;
  readonly requestTimeoutMs?: number;
  readonly observeEgress?: EgressObserver;
}

export interface ModelDownloadResponse {
  readonly status: number;
  readonly hasBody: boolean;
  header(name: string): string | null;
  read(): Promise<ReadableStreamReadResult<Uint8Array>>;
  cancel(): Promise<void>;
}

/** Owns the trusted HTTP boundary and response-stream timeouts for model downloads. */
export class ModelDownloadTransport {
  readonly #fetch: typeof fetch;
  readonly #urlFor: (model: ModelManifestEntry, file: ModelManifestFile) => string;
  readonly #validateRequestUrl: (url: string) => boolean;
  readonly #requestTimeoutMs: number;
  readonly #observeEgress: EgressObserver;

  constructor(options: ModelDownloadTransportOptions) {
    this.#fetch = options.fetch ?? fetch;
    this.#urlFor = options.urlFor ?? defaultModelUrl;
    this.#validateRequestUrl = options.validateRequestUrl ?? defaultValidateRequestUrl;
    this.#requestTimeoutMs = options.requestTimeoutMs ?? MODEL_DOWNLOAD_REQUEST_TIMEOUT_MS;
    this.#observeEgress = options.observeEgress ?? (() => undefined);
  }

  async request(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    offset: number,
    signal: AbortSignal,
  ): Promise<ModelDownloadResponse> {
    const headers = {
      'Accept-Encoding': 'identity',
      ...(offset > 0 ? { Range: `bytes=${String(offset)}-` } : {}),
    };
    const response = await this.#fetchWithRedirects(this.#urlFor(model, file), headers, signal);
    return new FetchModelDownloadResponse(response, this.#requestTimeoutMs, signal);
  }

  async #fetchWithRedirects(
    initialUrl: string,
    headers: Readonly<Record<string, string>>,
    activeSignal: AbortSignal,
  ): Promise<Response> {
    let current: URL;
    try {
      current = new URL(initialUrl);
    } catch {
      throw new ModelManagerError('PROTOCOL', 'Model download URL was invalid.');
    }
    for (let redirectCount = 0; redirectCount <= MODEL_DOWNLOAD_MAX_REDIRECTS; redirectCount += 1) {
      if (!this.#validateRequestUrl(current.href)) {
        throw new ModelManagerError('PROTOCOL', 'Model download destination is not trusted.');
      }
      const timeoutController = new AbortController();
      const timeout = setTimeout(
        () => timeoutController.abort('request timeout'),
        this.#requestTimeoutMs,
      );
      timeout.unref();
      const signal = AbortSignal.any([activeSignal, timeoutController.signal]);
      let response: Response;
      try {
        this.#observeEgress('model-download');
        response = await this.#fetch(current, { headers, redirect: 'manual', signal });
      } catch (error: unknown) {
        if (timeoutController.signal.aborted && !activeSignal.aborted) {
          throw new ModelManagerError('TIMEOUT', 'Model download request timed out.', true);
        }
        if (error instanceof TypeError) {
          throw new ModelManagerError(
            'OFFLINE',
            'The model host is unreachable. A completed cached model remains available offline.',
          );
        }
        throw error;
      } finally {
        clearTimeout(timeout);
      }
      if (![301, 302, 303, 307, 308].includes(response.status)) return response;
      const location = response.headers.get('location');
      if (location === null) {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Model download redirect had no destination.');
      }
      if (redirectCount === MODEL_DOWNLOAD_MAX_REDIRECTS) {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Model download exceeded the redirect limit.');
      }
      let next: URL;
      try {
        next = new URL(location, current);
      } catch {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Model download redirect was invalid.');
      }
      await cancelResponseBody(response);
      current = next;
    }
    throw new ModelManagerError('PROTOCOL', 'Model download redirect failed.');
  }
}

class FetchModelDownloadResponse implements ModelDownloadResponse {
  readonly #response: Response;
  readonly #timeoutMs: number;
  readonly #signal: AbortSignal;
  #reader: ReadableStreamDefaultReader<Uint8Array> | null = null;

  constructor(response: Response, timeoutMs: number, signal: AbortSignal) {
    this.#response = response;
    this.#timeoutMs = timeoutMs;
    this.#signal = signal;
  }

  get status(): number {
    return this.#response.status;
  }

  get hasBody(): boolean {
    return this.#response.body !== null;
  }

  header(name: string): string | null {
    return this.#response.headers.get(name);
  }

  read(): Promise<ReadableStreamReadResult<Uint8Array>> {
    const body = this.#response.body;
    if (body === null) {
      return Promise.reject(new ModelManagerError('PROTOCOL', 'Download response had no body.'));
    }
    this.#reader ??= body.getReader();
    return readWithInactivityTimeout(this.#reader, this.#timeoutMs, this.#signal);
  }

  async cancel(): Promise<void> {
    if (this.#reader === null) await cancelResponseBody(this.#response);
    else await this.#reader.cancel().catch(() => undefined);
  }
}

function readWithInactivityTimeout(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  timeoutMs: number,
  signal: AbortSignal,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  if (signal.aborted) {
    return Promise.reject(new ModelManagerError('CANCELLED', 'Model download was cancelled.'));
  }
  return new Promise((resolveRead, reject) => {
    let settled = false;
    const finish = (operation: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal.removeEventListener('abort', onAbort);
      operation();
    };
    const onAbort = () =>
      finish(() => reject(new ModelManagerError('CANCELLED', 'Model download was cancelled.')));
    const timer = setTimeout(
      () =>
        finish(() =>
          reject(new ModelManagerError('TIMEOUT', 'Model download became inactive.', true)),
        ),
      timeoutMs,
    );
    timer.unref();
    signal.addEventListener('abort', onAbort, { once: true });
    void reader.read().then(
      (result) => finish(() => resolveRead(result)),
      (error: unknown) =>
        finish(() =>
          reject(error instanceof Error ? error : new Error('Model response body failed.')),
        ),
    );
  });
}

async function cancelResponseBody(response: Response): Promise<void> {
  await response.body?.cancel().catch(() => undefined);
}

function defaultModelUrl(model: ModelManifestEntry, file: ModelManifestFile): string {
  const path = file.path.split('/').map(encodeURIComponent).join('/');
  return `https://huggingface.co/${model.id}/resolve/${model.revision}/${path}`;
}

function defaultValidateRequestUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === 'https:' &&
      (url.hostname === 'huggingface.co' ||
        url.hostname === 'hf.co' ||
        url.hostname.endsWith('.hf.co') ||
        url.hostname.endsWith('.huggingface.co') ||
        url.hostname.endsWith('.xethub.hf.co'))
    );
  } catch {
    return false;
  }
}
