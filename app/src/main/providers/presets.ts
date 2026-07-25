import type {
  Destination,
  OpenAICompatibleProviderId,
  ProviderField,
  VisionCapability,
} from '../../shared/schemas/providers';

export type EndpointNormalization =
  'preserve' | 'origin-v1' | 'origin-api-v1' | 'origin-engines-v1';
export type ModelListFormat =
  | 'openai'
  | 'lmstudio'
  | 'lemonade'
  | 'foundry'
  | 'omlx'
  | 'together'
  | 'openrouter'
  | 'novita'
  | 'ppio'
  | 'apipie'
  | 'cerebras'
  | 'privatemode'
  | 'docker';
export type ModelFilter =
  | 'all'
  | 'openai'
  | 'groq'
  | 'chat'
  | 'context-required'
  | 'no-embed'
  | 'no-whisper'
  | 'lemonade-llm'
  | 'cometapi'
  | 'privatemode-generate';

export interface OpenAICompatiblePreset {
  readonly id: OpenAICompatibleProviderId;
  readonly displayName: string;
  readonly description: string;
  readonly logo: string;
  readonly destinationHint: Destination;
  readonly endpoint: {
    readonly kind: 'fixed' | 'configurable';
    readonly value?: string;
    /** Display-only example for configurable endpoints; never a runtime fallback. */
    readonly example?: string;
    readonly normalization: EndpointNormalization;
  };
  readonly auth: 'none' | 'optional-bearer' | 'required-bearer';
  readonly protocol: 'responses' | 'chat-completions';
  readonly completionPath: string;
  readonly maxTokensField?: 'max_tokens' | 'max_completion_tokens';
  readonly temperatureMode?: 'requested' | 'openai-reasoning-one';
  readonly preparation?: 'lemonade-load';
  readonly modelList:
    | {
        readonly kind: 'http';
        readonly path: string;
        readonly format: ModelListFormat;
        readonly filter: ModelFilter;
        readonly public: boolean;
        readonly contextPath?: string;
      }
    | {
        readonly kind: 'static';
        readonly models: readonly {
          readonly id: string;
          readonly contextWindow: number;
        }[];
      }
    | { readonly kind: 'none' };
  readonly defaultModel: string | null;
  readonly defaultContextWindow: number;
  readonly defaultMaxOutputTokens: number;
  readonly modelContextWindows?: Readonly<Record<string, number>>;
  readonly vision: VisionCapability;
  readonly fields: readonly ProviderField[];
}

const urlField = (placeholder: string): ProviderField =>
  Object.freeze({
    key: 'baseUrl',
    label: 'Endpoint URL',
    kind: 'url',
    required: true,
    secret: false,
    placeholder,
  });
const credentialField = (required: boolean): ProviderField =>
  Object.freeze({
    key: 'credential',
    label: 'API key',
    kind: 'secret',
    required,
    secret: true,
  });
const modelField = (required = true): ProviderField =>
  Object.freeze({
    key: 'modelId',
    label: 'Model',
    kind: 'model',
    required,
    secret: false,
  });
const contextField = (required = false, defaultValue?: number): ProviderField =>
  Object.freeze({
    key: 'contextWindow',
    label: 'Context window',
    kind: 'number',
    required,
    secret: false,
    min: 1,
    ...(defaultValue === undefined ? {} : { defaultValue }),
  });
const outputField = (required = false, defaultValue?: number): ProviderField =>
  Object.freeze({
    key: 'maxOutputTokens',
    label: 'Maximum output tokens',
    kind: 'number',
    required,
    secret: false,
    min: 1,
    max: 16_384,
    ...(defaultValue === undefined ? {} : { defaultValue }),
  });
const timeoutField = (): ProviderField =>
  Object.freeze({
    key: 'timeoutMs',
    label: 'Request timeout',
    kind: 'number',
    required: false,
    secret: false,
    min: 500,
    max: 120_000,
    defaultValue: 30_000,
    description: 'Total deadline for a non-streaming provider request.',
  });

const fixed = (value: string) =>
  Object.freeze({ kind: 'fixed' as const, value, normalization: 'preserve' as const });
const configurable = (example: string, normalization: EndpointNormalization = 'preserve') =>
  Object.freeze({ kind: 'configurable' as const, example, normalization });
const httpModels = (
  path: string,
  format: ModelListFormat = 'openai',
  filter: ModelFilter = 'all',
  isPublic = false,
  contextPath?: string,
) =>
  Object.freeze({
    kind: 'http' as const,
    path,
    format,
    filter,
    public: isPublic,
    ...(contextPath === undefined ? {} : { contextPath }),
  });
const staticModels = (...models: readonly (readonly [id: string, contextWindow: number])[]) =>
  Object.freeze({
    kind: 'static' as const,
    models: Object.freeze(
      models.map(([id, contextWindow]) => Object.freeze({ id, contextWindow })),
    ),
  });
const noModels = Object.freeze({ kind: 'none' as const });
const fields = (...items: ProviderField[]) => Object.freeze(items);

const MODEL_CONTEXT_WINDOWS = {
  openai: {
    'gpt-3.5-turbo': 16_385,
    'gpt-3.5-turbo-1106': 16_385,
    'gpt-4o': 128_000,
    'gpt-4o-2024-08-06': 128_000,
    'gpt-4o-2024-05-13': 128_000,
    'gpt-4o-mini': 128_000,
    'gpt-4o-mini-2024-07-18': 128_000,
    'gpt-4-turbo': 128_000,
    'gpt-4-1106-preview': 128_000,
    'gpt-4-turbo-preview': 128_000,
    'gpt-4': 8_192,
    'gpt-4-32k': 32_000,
    'gpt-4.1': 1_047_576,
    'gpt-4.1-2025-04-14': 1_047_576,
    'gpt-4.1-mini': 1_047_576,
    'gpt-4.1-mini-2025-04-14': 1_047_576,
    'gpt-4.1-nano': 1_047_576,
    'gpt-4.1-nano-2025-04-14': 1_047_576,
    'gpt-4.5-preview': 128_000,
    'gpt-4.5-preview-2025-02-27': 128_000,
    'o1-preview': 128_000,
    'o1-preview-2024-09-12': 128_000,
    'o1-mini': 128_000,
    'o1-mini-2024-09-12': 128_000,
    o1: 200_000,
    'o1-2024-12-17': 200_000,
    'o1-pro': 200_000,
    'o1-pro-2025-03-19': 200_000,
    'o3-mini': 200_000,
    'o3-mini-2025-01-31': 200_000,
  },
  groq: {
    'gemma2-9b-it': 8_192,
    'gemma-7b-it': 8_192,
    'llama3-70b-8192': 8_192,
    'llama3-8b-8192': 8_192,
    'llama-3.1-70b-versatile': 8_000,
    'llama-3.1-8b-instant': 8_000,
    'mixtral-8x7b-32768': 32_768,
  },
  deepseek: {
    'deepseek-chat': 128_000,
    'deepseek-coder': 128_000,
    'deepseek-reasoner': 128_000,
  },
  minimax: {
    'MiniMax-M2.7': 196_000,
    'MiniMax-M2.7-highspeed': 196_000,
    'MiniMax-M2.5': 196_000,
    'MiniMax-M2.5-highspeed': 196_000,
    'MiniMax-M2.1': 196_000,
    'MiniMax-M2.1-highspeed': 196_000,
    'MiniMax-M2': 196_000,
  },
  cerebras: {
    'llama-3.3-70b': 128_000,
    'llama3.1-8b': 128_000,
    'gpt-oss-120b': 131_072,
    'qwen-3-32b': 128_000,
    'zai-glm-4.6': 128_000,
  },
  xai: { 'grok-beta': 131_072 },
  giteeai: {
    'Qwen2.5-72B-Instruct': 16_384,
    'Qwen2.5-14B-Instruct': 24_576,
    'Qwen2-7B-Instruct': 24_576,
    'Qwen2.5-32B-Instruct': 32_768,
    'Qwen2-72B-Instruct': 32_768,
    'Qwen2-VL-72B': 32_768,
    'QwQ-32B-Preview': 32_768,
    'Yi-34B-Chat': 4_096,
    'glm-4-9b-chat': 32_768,
    'deepseek-coder-33B-instruct': 8_192,
    'codegeex4-all-9b': 32_768,
    'InternVL2-8B': 32_768,
    'InternVL2.5-26B': 32_768,
    'InternVL2.5-78B': 32_768,
    'DeepSeek-R1-Distill-Qwen-32B': 32_768,
    'DeepSeek-R1-Distill-Qwen-1.5B': 32_768,
    'DeepSeek-R1-Distill-Qwen-14B': 32_768,
    'DeepSeek-R1-Distill-Qwen-7B': 32_768,
    'DeepSeek-V3': 32_768,
    'DeepSeek-R1': 32_768,
  },
  privatemode: {
    'gemma-3-27b': 128_000,
    'qwen3-coder-30b-a3b': 128_000,
    'gpt-oss-120b': 128_000,
  },
} as const;

const PRESET_DATA = [
  {
    id: 'openai',
    displayName: 'OpenAI',
    description: 'The standard option for most non-commercial use.',
    logo: 'openai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.openai.com/v1'),
    auth: 'required-bearer',
    protocol: 'responses',
    completionPath: 'responses',
    temperatureMode: 'openai-reasoning-one',
    modelList: httpModels('models', 'openai', 'openai'),
    defaultModel: 'gpt-4.1-nano',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.openai,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'generic-openai',
    displayName: 'Generic OpenAI',
    description: 'Connect to any OpenAI-compatible service via a custom configuration.',
    logo: 'generic-openai.png',
    destinationHint: 'local',
    endpoint: configurable('https://proxy.openai.com'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 1_024,
    vision: 'unknown',
    fields: fields(
      urlField('https://proxy.openai.com'),
      credentialField(false),
      modelField(),
      contextField(true, 4_096),
      outputField(true, 1_024),
    ),
  },
  {
    id: 'lmstudio',
    displayName: 'LM Studio',
    description: 'Discover, download, and run thousands of cutting edge LLMs in a few clicks.',
    logo: 'lmstudio.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:1234', 'origin-v1'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'lmstudio', 'no-embed', false, '/api/v1/models'),
    defaultModel: null,
    defaultContextWindow: 16_384,
    defaultMaxOutputTokens: 1_024,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:1234'),
      credentialField(false),
      modelField(),
      contextField(),
    ),
  },
  {
    id: 'localai',
    displayName: 'Local AI',
    description: 'Run LLMs locally on your own machine.',
    logo: 'localai.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:8080/v1'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 1_024,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:8080/v1'),
      credentialField(false),
      modelField(),
      contextField(true, 4_096),
    ),
  },
  {
    id: 'koboldcpp',
    displayName: 'KoboldCPP',
    description: 'Run local LLMs using koboldcpp.',
    logo: 'koboldcpp.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:5000/v1'),
    auth: 'none',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:5000/v1'),
      modelField(),
      contextField(true, 4_096),
      outputField(true, 2_048),
    ),
  },
  {
    id: 'textgenwebui',
    displayName: 'Oobabooga Web UI',
    description: "Run local LLMs using Oobabooga's Text Generation Web UI.",
    logo: 'text-generation-webui.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:5000/v1'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: noModels,
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 1_024,
    vision: 'unsupported',
    fields: fields(
      urlField('http://127.0.0.1:5000/v1'),
      credentialField(false),
      contextField(true, 4_096),
    ),
  },
  {
    id: 'docker-model-runner',
    displayName: 'Docker Model Runner',
    description: 'Run LLMs using Docker Model Runner.',
    logo: 'docker-model-runner.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:12434', 'origin-engines-v1'),
    auth: 'none',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('/models', 'docker', 'no-embed'),
    defaultModel: null,
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(urlField('http://127.0.0.1:12434'), modelField(), contextField(true, 8_192)),
  },
  {
    id: 'lemonade',
    displayName: 'Lemonade',
    description: 'Run local LLMs, ASR, TTS, and more in a single unified AI runtime.',
    logo: 'lemonade.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:13305', 'origin-api-v1'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    preparation: 'lemonade-load',
    // The default listing is installed-only; never request the downloadable catalog.
    modelList: httpModels('models', 'lemonade', 'lemonade-llm'),
    defaultModel: null,
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:13305'),
      credentialField(false),
      modelField(),
      contextField(true, 8_192),
    ),
  },
  {
    id: 'foundry',
    displayName: 'Microsoft Foundry Local',
    description: "Run Microsoft's Foundry models locally.",
    logo: 'foundry-local.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:8080', 'origin-v1'),
    auth: 'none',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    maxTokensField: 'max_completion_tokens',
    modelList: httpModels('models', 'foundry'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 1_024,
    vision: 'unknown',
    fields: fields(urlField('http://127.0.0.1:8080'), modelField(), contextField()),
  },
  {
    id: 'omlx',
    displayName: 'oMLX',
    description: 'Run MLX models on Apple Silicon with smart caching.',
    logo: 'omlx.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:8000', 'origin-v1'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'omlx'),
    defaultModel: null,
    defaultContextWindow: 16_000,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:8000'),
      credentialField(false),
      modelField(),
      contextField(),
    ),
  },
  {
    id: 'groq',
    displayName: 'Groq',
    description: 'The fastest LLM inferencing available for real-time AI applications.',
    logo: 'groq.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.groq.com/openai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openai', 'groq'),
    defaultModel: 'llama-3.1-8b-instant',
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.groq,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'openrouter',
    displayName: 'OpenRouter',
    description: 'A unified interface for LLMs.',
    logo: 'openrouter.jpeg',
    destinationHint: 'cloud',
    endpoint: fixed('https://openrouter.ai/api/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openrouter', 'all', true),
    defaultModel: 'openrouter/auto',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), timeoutField(), modelField()),
  },
  {
    id: 'togetherai',
    displayName: 'Together AI',
    description: 'Run open source models from Together AI.',
    logo: 'togetherai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.together.xyz/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'together', 'chat'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'fireworksai',
    displayName: 'Fireworks AI',
    description:
      'The fastest and most efficient inference engine to build production-ready, compound AI systems.',
    logo: 'fireworksai.jpeg',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.fireworks.ai/inference/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openai', 'context-required'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'deepseek',
    displayName: 'DeepSeek',
    description: "Run DeepSeek's powerful LLMs.",
    logo: 'deepseek.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.deepseek.com/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: 'deepseek-chat',
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.deepseek,
    vision: 'unsupported',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'perplexity',
    displayName: 'Perplexity AI',
    description: 'Run powerful and internet-connected models hosted by Perplexity AI.',
    logo: 'perplexity.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.perplexity.ai'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: staticModels(
      ['sonar', 127_072],
      ['sonar-pro', 200_000],
      ['sonar-reasoning', 127_072],
      ['sonar-reasoning-pro', 127_072],
    ),
    defaultModel: 'sonar',
    defaultContextWindow: 127_072,
    defaultMaxOutputTokens: 2_048,
    vision: 'unsupported',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'mistral',
    displayName: 'Mistral',
    description: 'Run open source models from Mistral AI.',
    logo: 'mistral.jpeg',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.mistral.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openai', 'no-embed'),
    defaultModel: 'mistral-tiny',
    defaultContextWindow: 32_000,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'novita',
    displayName: 'Novita AI',
    description: 'Reliable, Scalable, and Cost-Effective for LLMs from Novita AI',
    logo: 'novita.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.novita.ai/v3/openai'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'novita', 'all', true),
    defaultModel: 'deepseek/deepseek-r1',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), timeoutField(), modelField()),
  },
  {
    id: 'cometapi',
    displayName: 'CometAPI',
    description: '500+ AI Models all in one API.',
    logo: 'cometapi.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.cometapi.com/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openai', 'cometapi'),
    defaultModel: 'gpt-5-mini',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), timeoutField(), modelField()),
  },
  {
    id: 'ppio',
    displayName: 'PPIO',
    description:
      'Run stable and cost-efficient open-source LLM APIs, such as DeepSeek, Llama, Qwen etc.',
    logo: 'ppio.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.ppinfra.com/v3/openai'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'ppio'),
    defaultModel: 'qwen/qwen2.5-32b-instruct',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'apipie',
    displayName: 'APIpie',
    description: 'A unified API of AI services from leading providers',
    logo: 'apipie.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://apipie.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'apipie', 'chat'),
    defaultModel: 'openrouter/mistral-7b-instruct',
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'sambanova',
    displayName: 'SambaNova',
    description: 'Run open source models from SambaNova.',
    logo: 'sambanova.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.sambanova.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'openai', 'no-whisper'),
    defaultModel: null,
    defaultContextWindow: 131_072,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'cerebras',
    displayName: 'Cerebras',
    description: 'Run models at instant speed on Cerebras inference.',
    logo: 'cerebras.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.cerebras.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('/public/v1/models', 'cerebras', 'all', true),
    defaultModel: 'gpt-oss-120b',
    defaultContextWindow: 128_000,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.cerebras,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'giteeai',
    displayName: 'GiteeAI',
    description: "Run GiteeAI's powerful LLMs.",
    logo: 'giteeai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://ai.gitee.com/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models?type=text2text'),
    defaultModel: null,
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.giteeai,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField(), contextField(true, 8_192)),
  },
  {
    id: 'minimax',
    displayName: 'Minimax',
    description: "Run Minimax's powerful M2 LLMs.",
    logo: 'minimax.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.minimax.io/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: 'MiniMax-M2.7',
    defaultContextWindow: 196_000,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.minimax,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'moonshotai',
    displayName: 'Moonshot AI',
    description: "Run Moonshot AI's powerful LLMs.",
    logo: 'moonshotai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.moonshot.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: 'moonshot-v1-32k',
    defaultContextWindow: 8_192,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'zai',
    displayName: 'Z.AI',
    description: "Run Z.AI's powerful GLM models.",
    logo: 'zai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.z.ai/api/paas/v4'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: 'glm-4.5',
    defaultContextWindow: 131_072,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'xai',
    displayName: 'xAI',
    description: "Run xAI's powerful LLMs like Grok-2 and more.",
    logo: 'xai.png',
    destinationHint: 'cloud',
    endpoint: fixed('https://api.x.ai/v1'),
    auth: 'required-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: 'grok-beta',
    defaultContextWindow: 131_072,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.xai,
    vision: 'unknown',
    fields: fields(credentialField(true), modelField()),
  },
  {
    id: 'nvidia-nim',
    displayName: 'NVIDIA NIM',
    description: 'Run full parameter LLMs directly on your NVIDIA RTX GPU using NVIDIA NIM.',
    logo: 'nvidia-nim.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:8000', 'origin-v1'),
    auth: 'none',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'omlx'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 2_048,
    vision: 'unknown',
    fields: fields(urlField('http://127.0.0.1:8000'), modelField()),
  },
  {
    id: 'privatemode',
    displayName: 'Privatemode',
    description: 'Run LLMs with end-to-end encryption.',
    logo: 'privatemode.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:8080', 'origin-v1'),
    auth: 'none',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models', 'privatemode', 'privatemode-generate'),
    defaultModel: null,
    defaultContextWindow: 16_384,
    defaultMaxOutputTokens: 2_048,
    modelContextWindows: MODEL_CONTEXT_WINDOWS.privatemode,
    vision: 'unknown',
    fields: fields(urlField('http://127.0.0.1:8080'), modelField()),
  },
  {
    id: 'litellm',
    displayName: 'LiteLLM',
    description: "Run LiteLLM's OpenAI compatible proxy for various LLMs.",
    logo: 'litellm.png',
    destinationHint: 'local',
    endpoint: configurable('http://127.0.0.1:4000'),
    auth: 'optional-bearer',
    protocol: 'chat-completions',
    completionPath: 'chat/completions',
    modelList: httpModels('models'),
    defaultModel: null,
    defaultContextWindow: 4_096,
    defaultMaxOutputTokens: 1_024,
    vision: 'unknown',
    fields: fields(
      urlField('http://127.0.0.1:4000'),
      credentialField(false),
      modelField(),
      contextField(true, 4_096),
    ),
  },
] as const satisfies readonly OpenAICompatiblePreset[];

export const OPENAI_COMPATIBLE_PRESETS: readonly OpenAICompatiblePreset[] = deepFreeze(PRESET_DATA);

export function getOpenAICompatiblePreset(id: OpenAICompatibleProviderId): OpenAICompatiblePreset {
  const preset = OPENAI_COMPATIBLE_PRESETS.find((candidate) => candidate.id === id);
  if (preset === undefined) throw new Error('Provider preset is missing');
  return preset;
}

function deepFreeze<Value>(value: Value): Value {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const nested of Object.values(value)) deepFreeze(nested);
  }
  return value;
}
