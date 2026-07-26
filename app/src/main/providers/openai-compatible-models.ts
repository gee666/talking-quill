import {
  ModelInfoSchema,
  type ModelInfo,
  type VisionCapability,
} from '../../shared/schemas/providers';
import { ProviderError } from './errors';
import type { ModelFilter, ModelListFormat } from './presets';

const MAX_MODELS = 10_000;

interface ModelCandidate {
  readonly source: unknown;
  readonly modelId: string | null;
}

export function normalizeModels(
  input: unknown,
  format: ModelListFormat,
  filter: ModelFilter,
  fallbackContext: number,
  contextWindows: Readonly<Record<string, number>>,
  fallbackVision: VisionCapability,
): readonly ModelInfo[] {
  const candidates = modelCandidates(input, format);
  const seen = new Set<string>();
  const models: ModelInfo[] = [];
  for (const candidate of candidates) {
    const record = asRecord(candidate.source);
    if (record === null || !passesModelFilter(record, filter, format, candidate.modelId)) continue;
    const id = candidate.modelId === null ? modelId(record, format) : readString(candidate.modelId);
    if (id === null || seen.has(id) || hasControlCharacters(id)) continue;
    seen.add(id);
    const name = modelName(record, id, format);
    const loadedInstance = Array.isArray(record.loaded_instances)
      ? asRecord(record.loaded_instances[0])
      : null;
    const loadedConfig = asRecord(loadedInstance?.config);
    const contextWindow = readPositiveInteger(
      record.context_length ??
        loadedConfig?.context_length ??
        record.context_size ??
        record.max_model_len ??
        record.max_context_length ??
        record.max_tokens ??
        record.maxLength ??
        asRecord(record.limits)?.max_context_length ??
        readGgufContext(record) ??
        addPositiveIntegers(record.maxInputTokens, record.maxOutputTokens),
    );
    models.push(
      ModelInfoSchema.parse({
        id,
        name,
        contextWindow: contextWindow ?? contextWindows[id] ?? fallbackContext,
        vision: inferVision(record, fallbackVision),
      }),
    );
  }
  return Object.freeze(models);
}

function modelCandidates(input: unknown, format: ModelListFormat): readonly ModelCandidate[] {
  const candidates: readonly unknown[] = Array.isArray(input)
    ? (input as readonly unknown[])
    : candidatesFromRecord(input, format);
  if (format !== 'docker') {
    if (candidates.length > MAX_MODELS) throw new ProviderError('INVALID_RESPONSE');
    return candidates.map((source) => Object.freeze({ source, modelId: null }));
  }
  const models: ModelCandidate[] = [];
  for (const source of candidates) {
    const record = asRecord(source);
    if (record === null || !Array.isArray(record.tags)) continue;
    for (const tag of record.tags) {
      if (typeof tag !== 'string') continue;
      if (models.length >= MAX_MODELS) throw new ProviderError('INVALID_RESPONSE');
      models.push(Object.freeze({ source: record, modelId: tag }));
    }
  }
  return models;
}

function candidatesFromRecord(input: unknown, format: ModelListFormat): readonly unknown[] {
  const record = asRecord(input);
  if (record === null) throw new ProviderError('INVALID_RESPONSE');
  if (Array.isArray(record.data)) return record.data;
  if (Array.isArray(record.models)) return record.models;
  if (format === 'apipie' && Array.isArray(record.items)) return record.items;
  throw new ProviderError('INVALID_RESPONSE');
}

function modelId(
  record: Readonly<Record<string, unknown>>,
  format: ModelListFormat,
): string | null {
  if (format === 'apipie') {
    const provider = readString(record.provider);
    const model = readString(record.model ?? record.id);
    if (provider !== null && model !== null && !model.includes('/')) return `${provider}/${model}`;
  }
  return readString(record.id ?? record.key ?? record.name ?? record.model_name ?? record.model);
}

function modelName(
  record: Readonly<Record<string, unknown>>,
  id: string,
  format: ModelListFormat,
): string {
  if (format === 'docker') return id.includes('/') ? (id.split('/').at(-1) ?? id) : id;
  if (format === 'lemonade') {
    const organization = /^[a-z]+/i.exec(id)?.[0];
    return organization === undefined ? id : `${organization}:${id}`;
  }
  if (format === 'privatemode') {
    return id
      .split('-')
      .map((word) => `${word.charAt(0).toUpperCase()}${word.slice(1)}`)
      .join(' ');
  }
  return readString(record.display_name ?? record.title ?? record.name ?? record.label) ?? id;
}

function passesModelFilter(
  record: Readonly<Record<string, unknown>>,
  filter: ModelFilter,
  format: ModelListFormat,
  modelIdOverride: string | null,
): boolean {
  const id = (
    modelIdOverride ??
    readString(record.id ?? record.name ?? record.model) ??
    ''
  ).toLowerCase();
  if (filter === 'openai') {
    const owner = (readString(record.owned_by) ?? '').toLowerCase();
    const custom = owner.length > 0 && owner !== 'system' && !owner.includes('openai');
    return (
      custom ||
      ((id.includes('gpt') || id.startsWith('o')) &&
        !/(?:audio|realtime|image|moderation|transcri|instruct|vision)/.test(id) &&
        !id.startsWith('ft:'))
    );
  }
  if (filter === 'groq') return !id.includes('whisper') && !id.includes('tool-use');
  if (filter === 'chat') {
    if (format === 'together') return readString(record.type)?.toLowerCase() === 'chat';
    const subtype = readString(record.subtype)?.toLowerCase() ?? '';
    return subtype.includes('chat') || subtype.includes('chatx');
  }
  if (filter === 'context-required') return readPositiveInteger(record.context_length) !== null;
  if (filter === 'no-embed') {
    const type = readString(record.type)?.toLowerCase() ?? '';
    return type !== 'embeddings' && !id.includes('embed') && !/(?:^|[/:-])all-mini/i.test(id);
  }
  if (filter === 'no-whisper') return !id.startsWith('whisper');
  if (filter === 'lemonade-llm') {
    const labels = readStringArray(record.labels).map((label) => label.toLowerCase());
    return !labels.includes('embeddings') && !labels.includes('reranking');
  }
  if (filter === 'cometapi') {
    return !/(?:dall-?e|midjourney|mj_|stable-diffusion|sd-|flux-|playground-v|ideogram|recraft|black-forest-labs|stability-ai|sdxl|suno_|tts|whisper|runway|luma[_-]|veo|kling_|minimax_video|hunyuan-t1|embedding|search-gpts|files_retrieve|moderation|deepl)/.test(
      id,
    );
  }
  if (filter === 'privatemode-generate') {
    return !id.includes('/') && readStringArray(record.tasks).includes('generate');
  }
  return true;
}

function inferVision(
  record: Readonly<Record<string, unknown>>,
  fallback: VisionCapability,
): VisionCapability {
  const capabilityRecord = asRecord(record.capabilities);
  if (capabilityRecord?.vision === true) return 'supported';
  if (capabilityRecord?.vision === false) return 'unsupported';
  const values = [
    ...readStringArray(record.capabilities),
    ...readStringArray(record.modalities),
    ...readStringArray(record.labels),
  ];
  if (values.some((value) => /vision|image/i.test(value))) return 'supported';
  return fallback;
}

function asRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}

function hasControlCharacters(value: string): boolean {
  for (const character of value) {
    const code = character.charCodeAt(0);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

function readString(input: unknown): string | null {
  return typeof input === 'string' && input.trim().length > 0 && input.length <= 512
    ? input.trim()
    : null;
}

function readStringArray(input: unknown): readonly string[] {
  return Array.isArray(input)
    ? input.filter(
        (value): value is string =>
          typeof value === 'string' && value.length > 0 && value.length <= 128,
      )
    : [];
}

function readGgufContext(record: Readonly<Record<string, unknown>>): number | null {
  const config = asRecord(record.config);
  const gguf = asRecord(config?.gguf);
  if (gguf === null) return null;
  for (const [key, value] of Object.entries(gguf)) {
    if (key.endsWith('.context_length')) return readPositiveInteger(value);
  }
  return null;
}

function readPositiveInteger(input: unknown): number | null {
  return typeof input === 'number' && Number.isInteger(input) && input > 0 && input <= 2_000_000
    ? input
    : null;
}

function addPositiveIntegers(left: unknown, right: unknown): number | null {
  const first = readPositiveInteger(left);
  const second = readPositiveInteger(right);
  return first === null || second === null ? null : first + second;
}
