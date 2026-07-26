import { describe, expect, it } from 'vitest';
import type { OpenAICompatibleProviderId } from '../../app/src/shared/schemas/providers';
import { providerModelSelectionPolicy } from '../../app/src/shared/provider-model-selection';
import {
  OPENAI_COMPATIBLE_PRESETS,
  type EndpointNormalization,
} from '../../app/src/main/providers/presets';

type AuthStyle = 'none' | 'optional-bearer' | 'required-bearer';
type ExpectedPreset = readonly [
  id: OpenAICompatibleProviderId,
  endpoint: string,
  normalization: EndpointNormalization,
  auth: AuthStyle,
  modelList: string,
  defaultModel: string | null,
  contextWindow: number,
];

const EXPECTED_PRESETS: readonly ExpectedPreset[] = [
  [
    'openai',
    'https://api.openai.com/v1',
    'preserve',
    'required-bearer',
    'models',
    'gpt-4.1-nano',
    4_096,
  ],
  [
    'generic-openai',
    '~https://proxy.openai.com',
    'preserve',
    'optional-bearer',
    'models',
    null,
    4_096,
  ],
  ['lmstudio', '~http://127.0.0.1:1234', 'origin-v1', 'optional-bearer', 'models', null, 16_384],
  ['localai', '~http://127.0.0.1:8080/v1', 'preserve', 'optional-bearer', 'models', null, 4_096],
  ['koboldcpp', '~http://127.0.0.1:5000/v1', 'preserve', 'none', 'models', null, 4_096],
  ['textgenwebui', '~http://127.0.0.1:5000/v1', 'preserve', 'optional-bearer', 'none', null, 4_096],
  [
    'docker-model-runner',
    '~http://127.0.0.1:12434',
    'origin-engines-v1',
    'none',
    '/models',
    null,
    8_192,
  ],
  [
    'lemonade',
    '~http://127.0.0.1:13305',
    'origin-api-v1',
    'optional-bearer',
    'models',
    null,
    8_192,
  ],
  ['foundry', '~http://127.0.0.1:8080', 'origin-v1', 'none', 'models', null, 4_096],
  ['omlx', '~http://127.0.0.1:8000', 'origin-v1', 'optional-bearer', 'models', null, 16_000],
  [
    'groq',
    'https://api.groq.com/openai/v1',
    'preserve',
    'required-bearer',
    'models',
    'llama-3.1-8b-instant',
    8_192,
  ],
  [
    'openrouter',
    'https://openrouter.ai/api/v1',
    'preserve',
    'required-bearer',
    'models',
    'openrouter/auto',
    4_096,
  ],
  [
    'togetherai',
    'https://api.together.xyz/v1',
    'preserve',
    'required-bearer',
    'models',
    null,
    4_096,
  ],
  [
    'fireworksai',
    'https://api.fireworks.ai/inference/v1',
    'preserve',
    'required-bearer',
    'models',
    null,
    4_096,
  ],
  [
    'deepseek',
    'https://api.deepseek.com/v1',
    'preserve',
    'required-bearer',
    'models',
    'deepseek-chat',
    8_192,
  ],
  [
    'perplexity',
    'https://api.perplexity.ai',
    'preserve',
    'required-bearer',
    'static',
    'sonar',
    127_072,
  ],
  [
    'mistral',
    'https://api.mistral.ai/v1',
    'preserve',
    'required-bearer',
    'models',
    'mistral-tiny',
    32_000,
  ],
  [
    'novita',
    'https://api.novita.ai/v3/openai',
    'preserve',
    'required-bearer',
    'models',
    'deepseek/deepseek-r1',
    4_096,
  ],
  [
    'cometapi',
    'https://api.cometapi.com/v1',
    'preserve',
    'required-bearer',
    'models',
    'gpt-5-mini',
    4_096,
  ],
  [
    'ppio',
    'https://api.ppinfra.com/v3/openai',
    'preserve',
    'required-bearer',
    'models',
    'qwen/qwen2.5-32b-instruct',
    4_096,
  ],
  [
    'apipie',
    'https://apipie.ai/v1',
    'preserve',
    'required-bearer',
    'models',
    'openrouter/mistral-7b-instruct',
    4_096,
  ],
  [
    'sambanova',
    'https://api.sambanova.ai/v1',
    'preserve',
    'required-bearer',
    'models',
    null,
    131_072,
  ],
  [
    'cerebras',
    'https://api.cerebras.ai/v1',
    'preserve',
    'required-bearer',
    '/public/v1/models',
    'gpt-oss-120b',
    128_000,
  ],
  [
    'giteeai',
    'https://ai.gitee.com/v1',
    'preserve',
    'required-bearer',
    'models?type=text2text',
    null,
    8_192,
  ],
  [
    'minimax',
    'https://api.minimax.io/v1',
    'preserve',
    'required-bearer',
    'models',
    'MiniMax-M2.7',
    196_000,
  ],
  [
    'moonshotai',
    'https://api.moonshot.ai/v1',
    'preserve',
    'required-bearer',
    'models',
    'moonshot-v1-32k',
    8_192,
  ],
  [
    'zai',
    'https://api.z.ai/api/paas/v4',
    'preserve',
    'required-bearer',
    'models',
    'glm-4.5',
    131_072,
  ],
  ['xai', 'https://api.x.ai/v1', 'preserve', 'required-bearer', 'models', 'grok-beta', 131_072],
  ['nvidia-nim', '~http://127.0.0.1:8000', 'origin-v1', 'none', 'models', null, 4_096],
  ['privatemode', '~http://127.0.0.1:8080', 'origin-v1', 'none', 'models', null, 16_384],
  ['litellm', '~http://127.0.0.1:4000', 'preserve', 'optional-bearer', 'models', null, 4_096],
];

describe('provider preset fidelity matrix', () => {
  it('locks endpoint, normalization, auth, model-list, default-model, and context metadata', () => {
    expect(OPENAI_COMPATIBLE_PRESETS).toHaveLength(EXPECTED_PRESETS.length);
    for (const [
      id,
      endpoint,
      normalization,
      auth,
      modelList,
      defaultModel,
      contextWindow,
    ] of EXPECTED_PRESETS) {
      const preset = OPENAI_COMPATIBLE_PRESETS.find((candidate) => candidate.id === id);
      expect(preset, id).toBeDefined();
      if (preset === undefined) throw new Error('preset is missing');
      const actualEndpoint =
        preset.endpoint.kind === 'fixed'
          ? preset.endpoint.value
          : `~${String(preset.endpoint.example)}`;
      const actualModelList =
        preset.modelList.kind === 'http' ? preset.modelList.path : preset.modelList.kind;
      expect(actualEndpoint, `${id} endpoint`).toBe(endpoint);
      expect(preset.endpoint.normalization, `${id} normalization`).toBe(normalization);
      expect(preset.auth, `${id} auth`).toBe(auth);
      expect(actualModelList, `${id} model list`).toBe(modelList);
      expect(preset.defaultModel, `${id} default model`).toBe(defaultModel);
      expect(preset.defaultContextWindow, `${id} context`).toBe(contextWindow);
      expect(preset.destinationHint, `${id} destination`).toBe(
        preset.endpoint.kind === 'fixed' ? 'cloud' : 'local',
      );
      expect(preset.protocol, `${id} protocol`).toBe(
        id === 'openai' ? 'responses' : 'chat-completions',
      );
      expect(preset.completionPath, `${id} completion path`).toBe(
        id === 'openai' ? 'responses' : 'chat/completions',
      );
    }
  });

  it('keeps generated forms consistent with endpoint and auth contracts', () => {
    for (const preset of OPENAI_COMPATIBLE_PRESETS) {
      const keys = preset.fields.map(({ key }) => key);
      expect(keys.includes('baseUrl'), `${preset.id} base URL field`).toBe(
        preset.endpoint.kind === 'configurable',
      );
      expect(keys.includes('credential'), `${preset.id} credential field`).toBe(
        preset.auth !== 'none',
      );
      const credential = preset.fields.find(({ key }) => key === 'credential');
      expect(credential?.required ?? false, `${preset.id} credential requirement`).toBe(
        preset.auth === 'required-bearer',
      );
      const modelSelection = providerModelSelectionPolicy(preset.id);
      expect(
        preset.fields.some(({ key, required }) => key === 'modelId' && required),
        `${preset.id} model requirement`,
      ).toBe(modelSelection === 'required');
      expect(preset.modelList.kind === 'none', `${preset.id} provider-managed model`).toBe(
        modelSelection === 'provider-managed',
      );
      expect(preset.fields.every((field) => field.secret === (field.key === 'credential'))).toBe(
        true,
      );
    }
  });
});
