import { z } from 'zod';

const utf8ByteLength = (value: string): number => new TextEncoder().encode(value).byteLength;
const noControlCharacters = (value: string): boolean => {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint < 0x20 || codePoint === 0x7f) return false;
  }
  return true;
};
const normalizeCommandText = (value: string): string => {
  const compatibilityLetters: Readonly<Record<string, string>> = {
    Ł: 'L',
    ł: 'l',
    Đ: 'D',
    đ: 'd',
    Ø: 'O',
    ø: 'o',
    Æ: 'AE',
    æ: 'ae',
    Œ: 'OE',
    œ: 'oe',
    Ð: 'D',
    ð: 'd',
    Þ: 'TH',
    þ: 'th',
  };
  return Array.from(value)
    .map((character) => compatibilityLetters[character] ?? character)
    .join('')
    .normalize('NFKD')
    .replace(/(?<=\p{Script=Latin})\p{M}+/gu, '')
    .toLocaleLowerCase('en-US')
    .replace(/[\p{Pd}'’ʼ]+/gu, '')
    .replace(/\p{P}+/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim();
};

// Fully local snapshot of the released v27 on-disk contract. Nothing in this file imports a
// current settings, profile, provider, welcome, or shortcut schema, so future tightening cannot
// reinterpret the v27 migration boundary.
export const LegacyShortcutKeyV27Schema = z.enum([
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'O',
  'P',
  'Q',
  'R',
  'S',
  'T',
  'U',
  'V',
  'W',
  'X',
  'Y',
  'Z',
]);
const Modifiers = z
  .object({ ctrl: z.boolean(), alt: z.boolean(), shift: z.boolean(), meta: z.boolean() })
  .strict();
export const LegacyShortcutV27Schema = z
  .object({
    modifiers: Modifiers,
    keys: z
      .array(LegacyShortcutKeyV27Schema)
      .min(1)
      .max(26)
      .refine((keys) => new Set(keys).size === keys.length),
  })
  .strict()
  .refine(({ modifiers }) => modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.meta);
const BuiltInId = z.enum([
  'general',
  'prompt',
  'prompt-to-english',
  'markdown',
  'translate-to-english',
]);
const ProfileId = z.union([BuiltInId, z.uuid()]);
export type LegacyShortcutV27 = z.infer<typeof LegacyShortcutV27Schema>;
type LegacyShortcut = LegacyShortcutV27;

const ReleasedBuiltInShortcuts: readonly {
  readonly id: z.infer<typeof BuiltInId>;
  readonly shortcut: LegacyShortcut;
}[] = [
  { id: 'general', shortcut: releasedAltShortcut(['X']) },
  { id: 'prompt', shortcut: releasedAltShortcut(['X', 'P']) },
  { id: 'prompt-to-english', shortcut: releasedAltShortcut(['X', 'Q']) },
  { id: 'markdown', shortcut: releasedAltShortcut(['X', 'M']) },
  { id: 'translate-to-english', shortcut: releasedAltShortcut(['X', 'T']) },
];

export const LegacyDictationProfileV27Schema = z
  .object({
    id: ProfileId,
    name: z.string().trim().min(1).max(80),
    shortcut: LegacyShortcutV27Schema,
    processingMode: z.enum(['raw', 'smart']),
    smartPrompt: z.string().trim().max(4096).nullable(),
  })
  .strict();
export const LegacyDictationProfileListV27Schema = z
  .array(LegacyDictationProfileV27Schema)
  .min(5)
  .max(13)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    const shortcuts = new Set<string>();
    const priorProfiles: { readonly id: string; readonly shortcut: LegacyShortcut }[] = [];
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id))
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      ids.add(profile.id);
      if (releasedReservedBindingForProfile(profile.id, profile.shortcut))
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'The default built-in profile shortcuts are reserved for their owners.',
        });
      const identity = JSON.stringify(profile.shortcut);
      if (shortcuts.has(identity))
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Profile shortcuts must be distinct',
        });
      else if (
        priorProfiles.some(
          (candidate) =>
            releasedShortcutsConflict(candidate.shortcut, profile.shortcut) &&
            !releasedCanonicalFamilyPair(
              candidate.id,
              candidate.shortcut,
              profile.id,
              profile.shortcut,
            ),
        )
      )
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message:
            'Profile shortcuts with the same modifiers must not prefix one another outside the built-in default family',
        });
      shortcuts.add(identity);
      priorProfiles.push(profile);
    }
    for (const id of BuiltInId.options)
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
  });

function releasedAltShortcut(keys: LegacyShortcut['keys']): LegacyShortcut {
  return { modifiers: { ctrl: false, alt: true, shift: false, meta: false }, keys };
}

function releasedShortcutsEqual(left: LegacyShortcut, right: LegacyShortcut): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function releasedShortcutsConflict(left: LegacyShortcut, right: LegacyShortcut): boolean {
  if (JSON.stringify(left.modifiers) !== JSON.stringify(right.modifiers)) return false;
  const prefixLength = Math.min(left.keys.length, right.keys.length);
  return left.keys.slice(0, prefixLength).every((key, index) => key === right.keys[index]);
}

function releasedCanonicalOwner(id: string, shortcut: LegacyShortcut): boolean {
  return ReleasedBuiltInShortcuts.some(
    (candidate) => candidate.id === id && releasedShortcutsEqual(candidate.shortcut, shortcut),
  );
}

function releasedCanonicalFamilyPair(
  leftId: string,
  left: LegacyShortcut,
  rightId: string,
  right: LegacyShortcut,
): boolean {
  return (
    leftId !== rightId &&
    releasedCanonicalOwner(leftId, left) &&
    releasedCanonicalOwner(rightId, right)
  );
}

function releasedReservedBindingForProfile(id: string, shortcut: LegacyShortcut): boolean {
  if (releasedCanonicalOwner(id, shortcut)) return false;
  return ReleasedBuiltInShortcuts.some((candidate) =>
    releasedShortcutsConflict(candidate.shortcut, shortcut),
  );
}

export const LegacyProviderIdV27Schema = z.enum([
  'openai',
  'generic-openai',
  'lmstudio',
  'localai',
  'koboldcpp',
  'textgenwebui',
  'docker-model-runner',
  'lemonade',
  'foundry',
  'omlx',
  'groq',
  'openrouter',
  'togetherai',
  'fireworksai',
  'deepseek',
  'perplexity',
  'mistral',
  'novita',
  'cometapi',
  'ppio',
  'apipie',
  'sambanova',
  'cerebras',
  'giteeai',
  'minimax',
  'moonshotai',
  'zai',
  'xai',
  'nvidia-nim',
  'privatemode',
  'litellm',
  'ollama',
  'pi',
  'anthropic',
  'gemini',
  'azure',
  'bedrock',
  'cohere',
]);
const PersistedProviderBaseUrl = z
  .url()
  .max(2_048)
  .superRefine((value, context) => {
    if (!URL.canParse(value)) return;
    const endpoint = new URL(value);
    if (
      endpoint.username.length > 0 ||
      endpoint.password.length > 0 ||
      endpoint.search.length > 0 ||
      endpoint.hash.length > 0
    ) {
      context.addIssue({
        code: 'custom',
        message: 'Provider endpoints cannot contain credentials, queries, or fragments.',
      });
    }
  });
const PersistedPiExtensionSource = z
  .string()
  .trim()
  .min(1)
  .max(512)
  .regex(/^(?!-).+$/u)
  .refine(noControlCharacters, 'Extension paths cannot contain control characters.')
  .refine(
    (value) => !/^(?:[\\/]{2}|[\\/]\?\?[\\/])/u.test(value),
    'UNC, network, and namespaced paths cannot be used for Pi extensions.',
  );
export const LegacyProviderDraftV27Schema = z
  .object({
    baseUrl: PersistedProviderBaseUrl.optional(),
    modelId: z.string().trim().min(1).max(512).nullable().optional(),
    contextWindow: z.number().int().min(1).max(2_000_000).optional(),
    maxOutputTokens: z.number().int().min(1).max(16_384).optional(),
    timeoutMs: z.number().int().min(500).max(120_000).optional(),
    keepAlive: z
      .union([
        z.number().int().min(-1).max(86_400),
        z
          .string()
          .trim()
          .min(1)
          .max(32)
          .regex(/^(?:\d+(?:\.\d+)?(?:ns|us|µs|ms|s|m|h))+$/u),
      ])
      .optional(),
    region: z
      .string()
      .regex(
        /^(?:af|ap|ca|eu|il|me|mx|sa|us)-(?:central|east|north|northeast|northwest|south|southeast|southwest|west)-\d$/,
      )
      .max(32)
      .optional(),
    modelType: z.enum(['default', 'reasoning']).optional(),
    thinking: z.enum(['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']).optional(),
    piExtensionSources: z.array(PersistedPiExtensionSource).max(8).optional(),
  })
  .strict();
const configurableProviderIds = new Set([
  'generic-openai',
  'lmstudio',
  'localai',
  'koboldcpp',
  'textgenwebui',
  'docker-model-runner',
  'lemonade',
  'foundry',
  'omlx',
  'nvidia-nim',
  'privatemode',
  'litellm',
  'ollama',
  'azure',
]);
const timeoutProviderIds = new Set(['openrouter', 'novita', 'cometapi']);
const ProviderDrafts = z
  .partialRecord(LegacyProviderIdV27Schema, LegacyProviderDraftV27Schema)
  .superRefine((drafts, context) => {
    for (const [providerId, draft] of Object.entries(drafts)) {
      const issues: [string, string][] = [];
      if (configurableProviderIds.has(providerId) && draft.baseUrl === undefined)
        issues.push(['baseUrl', 'A provider endpoint is required.']);
      if (!configurableProviderIds.has(providerId) && draft.baseUrl !== undefined)
        issues.push(['baseUrl', 'Fixed providers do not accept endpoint overrides.']);
      if (draft.keepAlive !== undefined && providerId !== 'ollama')
        issues.push(['keepAlive', 'Keep alive is only supported by Ollama.']);
      if (draft.timeoutMs !== undefined && !timeoutProviderIds.has(providerId))
        issues.push(['timeoutMs', 'A custom timeout is not supported by this provider.']);
      if ((draft.region !== undefined) !== (providerId === 'bedrock'))
        issues.push(['region', 'Region is required only for AWS Bedrock.']);
      if (draft.modelType !== undefined && providerId !== 'azure')
        issues.push(['modelType', 'Model type is supported only by Azure OpenAI.']);
      if (draft.thinking !== undefined && providerId !== 'pi')
        issues.push(['thinking', 'Thinking level is required only for Pi.']);
      if (draft.piExtensionSources !== undefined && providerId !== 'pi')
        issues.push(['piExtensionSources', 'Extension sources are supported only by Pi.']);
      for (const [field, message] of issues)
        context.addIssue({ code: 'custom', path: [providerId, field], message });
    }
  });
export const LegacySmartProcessingV27Schema = z
  .object({
    selectedProviderId: LegacyProviderIdV27Schema,
    providers: ProviderDrafts,
    credentialEpochs: z.partialRecord(
      LegacyProviderIdV27Schema,
      z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
    ),
    piInstallationPath: z.string().trim().min(1).max(8192).nullable(),
    onScreenAwarenessEnabled: z.boolean(),
    visionOverrides: z
      .array(
        z
          .object({
            providerId: z.enum(['generic-openai', 'litellm']),
            binding: z.string().min(1).max(2048),
            modelId: z.string().trim().min(1).max(512),
            verifiedAt: z.number().int().nonnegative(),
          })
          .strict(),
      )
      .max(64),
  })
  .strict();

const VoiceCommandTrigger = z
  .string()
  .trim()
  .min(1)
  .max(200)
  .refine(
    (value) => value.length > 200 || utf8ByteLength(value) <= 400,
    'Trigger is too large when encoded as UTF-8',
  )
  .refine(noControlCharacters, 'Control characters are not allowed')
  .refine(
    (value) => /[\p{L}\p{N}]/u.test(normalizeCommandText(value)),
    'Trigger must contain a letter or number after normalization',
  );
const VoiceCommandSnippet = z
  .string()
  .min(1)
  .max(100_000)
  .refine(
    (value) => value.length > 100_000 || utf8ByteLength(value) <= 200_000,
    'Snippet is too large when encoded as UTF-8',
  )
  .refine((value) => value.trim().length > 0, 'Snippet must contain text')
  .refine(
    (value) =>
      !Array.from(value).some((character) => {
        const code = character.codePointAt(0) ?? 0;
        return code !== 9 && code !== 10 && code !== 13 && (code < 32 || code === 127);
      }),
    'Unsupported control characters are not allowed',
  );
const VoiceCommand = z
  .object({
    id: z.uuid(),
    trigger: VoiceCommandTrigger,
    snippet: VoiceCommandSnippet,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
const VoiceCommands = z
  .array(VoiceCommand)
  .max(100)
  .refine((commands) => {
    if (commands.length > 100) return true;
    let total = 0;
    for (const command of commands) {
      if (command.trigger.length > 200 || command.snippet.length > 100_000) return true;
      total += utf8ByteLength(command.trigger) + utf8ByteLength(command.snippet);
      if (total > 512_000) return false;
    }
    return true;
  }, 'Voice commands exceed the total UTF-8 size limit');
const VocabularyValue = z
  .string()
  .trim()
  .min(1)
  .max(200)
  .refine(
    (value) => value.length > 200 || utf8ByteLength(value) <= 400,
    'Vocabulary entry is too large when encoded as UTF-8',
  )
  .refine((value) => /[\p{L}\p{N}]/u.test(value), 'Vocabulary must contain a letter or number')
  .refine(noControlCharacters, 'Control characters are not allowed');
const Vocabulary = z
  .object({
    id: z.uuid(),
    value: VocabularyValue,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
const VocabularyList = z
  .array(Vocabulary)
  .max(1_000)
  .refine((entries) => {
    if (entries.length > 1_000) return true;
    let total = 0;
    for (const entry of entries) {
      if (entry.value.length > 200) return true;
      total += utf8ByteLength(entry.value);
      if (total > 256_000) return false;
    }
    return true;
  }, 'Custom vocabulary exceeds the total UTF-8 size limit');
const ModelId = z.enum(['onnx-community/whisper-large-v3-turbo', 'Xenova/whisper-small']);
const TranscriptionLanguage = z.enum([
  'auto',
  'en',
  'zh',
  'de',
  'es',
  'ru',
  'ko',
  'fr',
  'ja',
  'pt',
  'tr',
  'pl',
  'ca',
  'nl',
  'ar',
  'sv',
  'it',
  'id',
  'hi',
  'fi',
  'vi',
  'he',
  'uk',
  'el',
  'ms',
  'cs',
  'ro',
  'da',
  'hu',
  'ta',
  'no',
  'th',
  'ur',
  'hr',
  'bg',
  'lt',
  'la',
  'mi',
  'ml',
  'cy',
  'sk',
  'te',
  'fa',
  'lv',
  'bn',
  'sr',
  'az',
  'sl',
  'kn',
  'et',
  'mk',
  'br',
  'eu',
  'is',
  'hy',
  'ne',
  'mn',
  'bs',
  'kk',
  'sq',
  'sw',
  'gl',
  'mr',
  'pa',
  'si',
  'km',
  'sn',
  'yo',
  'so',
  'af',
  'oc',
  'ka',
  'be',
  'tg',
  'sd',
  'gu',
  'am',
  'yi',
  'lo',
  'uz',
  'fo',
  'ht',
  'ps',
  'tk',
  'nn',
  'mt',
  'sa',
  'lb',
  'my',
  'bo',
  'tl',
  'mg',
  'as',
  'tt',
  'haw',
  'ln',
  'ha',
  'ba',
  'jw',
  'su',
]);
const MicrophoneEvidence = z
  .object({
    boundDeviceId: z.string().min(1).max(4096).nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold);
const ActivationEvidence = z
  .object({
    profileId: ProfileId,
    activationKey: LegacyShortcutKeyV27Schema,
    shift: z.boolean(),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const ModelEvidence = z
  .object({
    modelId: ModelId,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
const Welcome = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: z.union([z.literal(1), z.literal(2), z.literal(3), z.literal(4), z.literal(5)]),
    microphoneTested: z.boolean(),
    activationTested: z.boolean(),
    microphoneEvidence: MicrophoneEvidence.nullable().optional(),
    activationEvidence: ActivationEvidence.nullable().optional(),
    modelEvidence: ModelEvidence.nullable().optional(),
    revision: z.number().int().nonnegative().optional(),
  })
  .strict();

export const LegacySettingsV27ObjectSchema = z
  .object({
    schemaVersion: z.literal(27),
    app: z
      .object({
        enabled: z.boolean(),
        closeToTray: z.boolean(),
        defaultProcessingMode: z.enum(['raw', 'smart']),
        widgetSize: z.enum(['default', 'large', 'huge', 'max']),
        soundsEnabled: z.boolean(),
        launchAtLogin: z.boolean(),
      })
      .strict(),
    recording: z
      .object({
        preferredMicrophoneId: z
          .string()
          .min(1)
          .max(4096)
          .refine((id) => id !== 'default')
          .nullable(),
        silencePreset: z.enum(['aggressive', 'average', 'relaxed']),
        autoSubmitOnSilence: z.boolean(),
        includeSystemAudio: z.boolean(),
      })
      .strict(),
    transcription: z.object({ modelId: ModelId, language: TranscriptionLanguage }).strict(),
    dictationProfiles: LegacyDictationProfileListV27Schema,
    privacy: z
      .object({
        historyEnabled: z.boolean(),
        historyRetentionDays: z.union([z.literal(7), z.literal(30), z.literal(90)]).nullable(),
        retainSmartScreenshots: z.boolean(),
        diagnosticLoggingEnabled: z.boolean(),
      })
      .strict(),
    smartProcessing: LegacySmartProcessingV27Schema,
    voiceCommands: VoiceCommands,
    customVocabulary: VocabularyList,
    welcome: Welcome,
  })
  .strict();

export const LegacySettingsV27Schema = LegacySettingsV27ObjectSchema.superRefine(
  (settings, context) => {
    const general = settings.dictationProfiles.find(({ id }) => id === 'general');
    if (general !== undefined && general.processingMode !== settings.app.defaultProcessingMode) {
      context.addIssue({
        code: 'custom',
        path: ['app', 'defaultProcessingMode'],
        message: 'Processing mirror must match General',
      });
    }
  },
);

export type LegacySettingsV27 = z.infer<typeof LegacySettingsV27Schema>;
