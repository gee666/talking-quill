import { ProviderError } from './errors';
import type { JsonTransport } from './json-transport';

interface LemonadePreparationOptions {
  readonly model: string | null | undefined;
  readonly contextWindow: number;
  readonly listInstalledModels: () => Promise<readonly { readonly id: string }[]>;
  readonly endpoint: (path: string) => URL;
  readonly transport: JsonTransport;
  readonly headers: Readonly<Record<string, string>>;
  readonly credentialed: boolean;
  readonly fixedCloud: boolean;
  readonly signal: AbortSignal;
  readonly timeoutMs?: number;
}

export async function prepareLemonadeCompletion(
  options: LemonadePreparationOptions,
): Promise<void> {
  const model = options.model;
  if (model === null || model === undefined) throw new ProviderError('INVALID_CONFIG');
  const installedModels = await options.listInstalledModels();
  if (!installedModels.some((installed) => installed.id === model)) {
    throw new ProviderError('MODEL_NOT_FOUND');
  }
  const health = await options.transport.request({
    url: options.endpoint('health'),
    method: 'GET',
    kind: 'model-detail',
    headers: options.headers,
    credentialed: options.credentialed,
    fixedCloud: options.fixedCloud,
    signal: options.signal,
    ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
    maxResponseBytes: 512 * 1024,
  });
  const healthRecord = asRecord(health.body);
  const loaded = healthRecord?.all_models_loaded;
  if (loaded !== undefined && !Array.isArray(loaded)) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (
    Array.isArray(loaded) &&
    loaded.slice(0, 256).some((item) => {
      const record = asRecord(item);
      const recipe = asRecord(record?.recipe_options);
      return (
        readString(record?.model_name) === model &&
        readPositiveInteger(recipe?.ctx_size) === options.contextWindow
      );
    })
  ) {
    return;
  }
  const load = await options.transport.request({
    url: options.endpoint('load'),
    method: 'POST',
    kind: 'model-detail',
    headers: options.headers,
    body: Object.freeze({ model_name: model, ctx_size: options.contextWindow }),
    credentialed: options.credentialed,
    fixedCloud: options.fixedCloud,
    signal: options.signal,
    ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
    maxResponseBytes: 512 * 1024,
  });
  if (asRecord(load.body)?.status !== 'success') {
    throw new ProviderError('INVALID_RESPONSE');
  }
}

function asRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}

function readString(input: unknown): string | null {
  return typeof input === 'string' && input.trim().length > 0 && input.length <= 512
    ? input.trim()
    : null;
}

function readPositiveInteger(input: unknown): number | null {
  return typeof input === 'number' && Number.isInteger(input) && input > 0 && input <= 2_000_000
    ? input
    : null;
}
