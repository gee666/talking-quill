import type { ProviderConfig, ProviderId, VisionCapability } from '../../shared/schemas/providers';

interface ProviderVisionTable {
  readonly supported: readonly RegExp[];
  readonly unsupported: readonly RegExp[];
}

// Model names are meaningful only within their provider namespace. Never infer a vendor clone's
// capabilities from another provider's naming convention.
const STATIC_VISION_TABLE: Readonly<Partial<Record<ProviderId, ProviderVisionTable>>> = {
  openai: {
    supported: [
      /^(?:gpt-4o|gpt-4\.1|gpt-4\.5|gpt-5)(?:[.:-]|$)/i,
      /^(?:o1|o3)(?:[.:-](?!mini(?:[.:-]|$))|$)/i,
    ],
    unsupported: [
      /^(?:o1-mini|o3-mini|gpt-3\.5|gpt-4-\d{4}|text-embedding|whisper|tts|dall-e)(?:[.:-]|$)/i,
    ],
  },
  groq: {
    supported: [/^(?:meta-llama\/)?llama-4-(?:scout|maverick)(?:[.:-]|$)/i],
    unsupported: [/^(?:whisper|distil-whisper)(?:[.:-]|$)/i],
  },
  openrouter: {
    supported: [/(?:^|\/)(?:llama-3\.2-vision|qwen2(?:\.5)?-vl|qwen3-vl)(?:[.:-]|$)/i],
    unsupported: [/^(?:text-embedding|embed)(?:[.:-]|$)/i],
  },
  togetherai: {
    supported: [/(?:^|\/)(?:llama-3\.2-vision|qwen2(?:\.5)?-vl)(?:[.:-]|$)/i],
    unsupported: [/^(?:text-embedding|embed)(?:[.:-]|$)/i],
  },
  fireworksai: {
    supported: [/(?:^|\/)(?:llama-3\.2-vision|qwen2(?:\.5)?-vl)(?:[.:-]|$)/i],
    unsupported: [/^(?:text-embedding|embed)(?:[.:-]|$)/i],
  },
  mistral: {
    supported: [/^(?:pixtral|mistral-small-3\.1)(?:[.:-]|$)/i],
    unsupported: [/^(?:mistral-embed)(?:[.:-]|$)/i],
  },
};

export const MANUAL_VISION_PROVIDER_IDS = Object.freeze([
  'generic-openai',
  'litellm',
] as const satisfies readonly ProviderId[]);

export interface VisionOverride {
  readonly providerId: 'generic-openai' | 'litellm';
  readonly binding: string;
  readonly modelId: string;
  readonly verifiedAt: number;
}

export function staticVisionCapability(providerId: ProviderId, modelId: string): VisionCapability {
  const table = STATIC_VISION_TABLE[providerId];
  if (table === undefined) return 'unknown';
  if (table.unsupported.some((pattern) => pattern.test(modelId))) return 'unsupported';
  if (table.supported.some((pattern) => pattern.test(modelId))) return 'supported';
  return 'unknown';
}

export function resolveVisionCapability(options: {
  readonly providerCapability: VisionCapability;
  readonly providerId: ProviderId;
  readonly modelId: string;
  readonly binding: string;
  readonly overrides?: readonly VisionOverride[];
}): VisionCapability {
  if (options.providerCapability === 'unsupported') return 'unsupported';
  const manualProvider = MANUAL_VISION_PROVIDER_IDS.includes(
    options.providerId as 'generic-openai' | 'litellm',
  );
  if (!manualProvider) {
    if (options.providerCapability === 'supported') return 'supported';
    return staticVisionCapability(options.providerId, options.modelId);
  }
  // Generic destinations can clone vendor names or return optimistic metadata. Image support is
  // enabled only by the destination/model/credential-bound live echo confirmation.
  return options.overrides?.some(
    (override) =>
      override.providerId === options.providerId &&
      override.binding === options.binding &&
      override.modelId === options.modelId,
  )
    ? 'supported'
    : 'unknown';
}

export function visionBinding(config: ProviderConfig): string {
  return `${config.providerId}\n${config.baseUrl ?? 'fixed'}`;
}
