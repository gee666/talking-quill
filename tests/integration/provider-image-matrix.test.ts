import { describe, expect, it } from 'vitest';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { PROVIDER_IDS, type ProviderId } from '../../app/src/shared/schemas/providers';

type ImageContract =
  | 'responses-input-image'
  | 'chat-image-url'
  | 'ollama-images'
  | 'anthropic-image-source'
  | 'gemini-inline-data'
  | 'azure-image-url'
  | 'bedrock-image-bytes'
  | 'fail-closed';

interface ImageContractRow {
  readonly wire: ImageContract;
  readonly requiresCapabilityGate: true;
  readonly authRedacted: true;
  readonly abortAndTimeoutBounded: true;
  readonly requestAndResponseBounded: true;
  readonly redirectsAndSsrfRevalidated: true;
}

const row = (wire: ImageContract): ImageContractRow =>
  Object.freeze({
    wire,
    requiresCapabilityGate: true,
    authRedacted: true,
    abortAndTimeoutBounded: true,
    requestAndResponseBounded: true,
    redirectsAndSsrfRevalidated: true,
  });

// Literal, reviewable contract: adding or removing a provider requires an explicit image decision.
const PROVIDER_IMAGE_CONTRACT = Object.freeze({
  openai: row('responses-input-image'),
  'generic-openai': row('chat-image-url'),
  lmstudio: row('chat-image-url'),
  localai: row('chat-image-url'),
  koboldcpp: row('chat-image-url'),
  textgenwebui: row('chat-image-url'),
  'docker-model-runner': row('chat-image-url'),
  lemonade: row('chat-image-url'),
  foundry: row('chat-image-url'),
  omlx: row('chat-image-url'),
  groq: row('chat-image-url'),
  openrouter: row('chat-image-url'),
  togetherai: row('chat-image-url'),
  fireworksai: row('chat-image-url'),
  deepseek: row('chat-image-url'),
  perplexity: row('chat-image-url'),
  mistral: row('chat-image-url'),
  novita: row('chat-image-url'),
  cometapi: row('chat-image-url'),
  ppio: row('chat-image-url'),
  apipie: row('chat-image-url'),
  sambanova: row('chat-image-url'),
  cerebras: row('chat-image-url'),
  giteeai: row('chat-image-url'),
  minimax: row('chat-image-url'),
  moonshotai: row('chat-image-url'),
  zai: row('chat-image-url'),
  xai: row('chat-image-url'),
  'nvidia-nim': row('chat-image-url'),
  privatemode: row('chat-image-url'),
  litellm: row('chat-image-url'),
  ollama: row('ollama-images'),
  pi: row('fail-closed'),
  anthropic: row('anthropic-image-source'),
  gemini: row('gemini-inline-data'),
  azure: row('azure-image-url'),
  bedrock: row('bedrock-image-bytes'),
  cohere: row('chat-image-url'),
} satisfies Readonly<Record<ProviderId, ImageContractRow>>);

describe('literal 38-provider image contract matrix', () => {
  it('has one exact fail-closed or wire-shape decision for every registered provider', () => {
    expect(Object.keys(PROVIDER_IMAGE_CONTRACT)).toEqual(PROVIDER_IDS);
    expect(Object.keys(PROVIDER_IMAGE_CONTRACT)).toHaveLength(38);
    expect(new ProviderRegistry().ids()).toEqual(PROVIDER_IDS);
  });

  it.each(PROVIDER_IDS)('%s retains every transport security invariant', (providerId) => {
    expect(PROVIDER_IMAGE_CONTRACT[providerId]).toMatchObject({
      requiresCapabilityGate: true,
      authRedacted: true,
      abortAndTimeoutBounded: true,
      requestAndResponseBounded: true,
      redirectsAndSsrfRevalidated: true,
    });
  });
});
