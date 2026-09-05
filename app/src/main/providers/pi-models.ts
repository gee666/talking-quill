import {
  ModelInfoSchema,
  ProviderConfigSchema,
  type ModelInfo,
  type ProviderConfig,
} from '../../shared/schemas/providers';
import { ProviderError } from './errors';

export const MAX_PI_STDOUT_BYTES = 2 * 1024 * 1024;
const MAX_STDOUT_BYTES = MAX_PI_STDOUT_BYTES;
const MAX_MODELS = 5_000;
const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._:@+-]{0,127}\/[A-Za-z0-9][A-Za-z0-9._:@+-]{0,383}$/u;

export type PiConfig = ProviderConfig & {
  readonly providerId: 'pi';
  readonly modelId: string;
  readonly thinking: NonNullable<ProviderConfig['thinking']>;
};

export function basePiConfig(
  input: ProviderConfig,
): ProviderConfig & { readonly providerId: 'pi' } {
  try {
    const config = ProviderConfigSchema.parse(input);
    if (config.providerId !== 'pi') throw new ProviderError('INVALID_CONFIG');
    return config as ProviderConfig & { readonly providerId: 'pi' };
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('INVALID_CONFIG');
  }
}

export function runtimePiConfig(input: ProviderConfig): PiConfig {
  const config = basePiConfig(input);
  if (config.modelId == null || config.thinking === undefined)
    throw new ProviderError('INVALID_CONFIG');
  assertModelId(config.modelId);
  return config as PiConfig;
}

export function parsePiModels(output: string): readonly ModelInfo[] {
  if (Buffer.byteLength(output, 'utf8') > MAX_STDOUT_BYTES)
    throw new ProviderError('RESPONSE_TOO_LARGE');
  const lines = output
    .replace(/^\uFEFF/u, '')
    .replace(new RegExp(`${String.fromCharCode(27)}\\[[0-?]*[ -/]*[@-~]`, 'gu'), '')
    .replace(/\r/g, '')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0 || /^no models/iu.test(lines[0] ?? '')) return Object.freeze([]);
  const headerIndex = lines.findIndex((line) => /provider/iu.test(line) && /model/iu.test(line));
  if (headerIndex < 0) return Object.freeze([]);
  const header = (lines[headerIndex] ?? '').toLowerCase().split(/\s{2,}|\t+|\s+/u);
  const providerIndex = header.indexOf('provider');
  const modelIndex = header.indexOf('model');
  const contextIndex = header.findIndex((value) => value.startsWith('context'));
  const imagesIndex = header.findIndex((value) => value.startsWith('image'));
  if (providerIndex < 0 || modelIndex < 0) return Object.freeze([]);
  const models: ModelInfo[] = [];
  const seen = new Set<string>();
  for (const line of lines.slice(headerIndex + 1)) {
    const columns = line.split(/\s{2,}|\t+/u).filter(Boolean);
    const fallback =
      columns.length < Math.max(providerIndex, modelIndex) + 1 ? line.split(/\s+/u) : columns;
    const provider = fallback[providerIndex];
    const model = fallback[modelIndex];
    if (provider === undefined || model === undefined) continue;
    const id = model.includes('/') ? model : `${provider}/${model}`;
    if (!MODEL_ID.test(id) || seen.has(id)) continue;
    let contextWindow: number | null = null;
    if (contextIndex >= 0 && fallback[contextIndex] !== undefined)
      contextWindow = parseCompactTokens(fallback[contextIndex] ?? '');
    const vision =
      imagesIndex >= 0 && /^(?:yes|true|supported)$/iu.test(fallback[imagesIndex] ?? '')
        ? 'supported'
        : 'unsupported';
    const parsed = ModelInfoSchema.safeParse({ id, name: id, contextWindow, vision });
    if (parsed.success) {
      seen.add(id);
      models.push(parsed.data);
    }
    if (models.length >= MAX_MODELS) break;
  }
  return Object.freeze(models.sort((left, right) => left.id.localeCompare(right.id)));
}

function parseCompactTokens(value: string): number | null {
  const match = /^(\d+(?:\.\d+)?)([kKmM])?$/u.exec(value);
  if (match?.[1] === undefined) return null;
  const multiplier =
    match[2]?.toLowerCase() === 'm' ? 1_000_000 : match[2]?.toLowerCase() === 'k' ? 1_000 : 1;
  const result = Number(match[1]) * multiplier;
  return Number.isSafeInteger(result) && result > 0 && result <= 2_000_000 ? result : null;
}
export function assertModelId(modelId: string): void {
  if (!MODEL_ID.test(modelId)) throw new ProviderError('INVALID_CONFIG');
}

export function splitPiModelId(modelId: string): readonly [string, string] {
  assertModelId(modelId);
  const separator = modelId.indexOf('/');
  return Object.freeze([modelId.slice(0, separator), modelId.slice(separator + 1)]);
}

export function freezePiConfig(config: PiConfig): PiConfig {
  return Object.freeze({
    ...config,
    ...(config.piExtensionSources === undefined
      ? {}
      : { piExtensionSources: Object.freeze([...config.piExtensionSources]) }),
  }) as PiConfig;
}
