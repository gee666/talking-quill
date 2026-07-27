import { z } from 'zod';

// Keep this inventory aligned with Transformers.js 3.8.1
// src/models/whisper/common_whisper.js. These values are optional source-language
// hints; omitting the hint lets Whisper detect the spoken language.
export const WHISPER_AUTO_LANGUAGE = 'auto' as const;

export const WHISPER_SOURCE_LANGUAGES = [
  ['en', 'English'],
  ['zh', 'Chinese'],
  ['de', 'German'],
  ['es', 'Spanish'],
  ['ru', 'Russian'],
  ['ko', 'Korean'],
  ['fr', 'French'],
  ['ja', 'Japanese'],
  ['pt', 'Portuguese'],
  ['tr', 'Turkish'],
  ['pl', 'Polish'],
  ['ca', 'Catalan'],
  ['nl', 'Dutch'],
  ['ar', 'Arabic'],
  ['sv', 'Swedish'],
  ['it', 'Italian'],
  ['id', 'Indonesian'],
  ['hi', 'Hindi'],
  ['fi', 'Finnish'],
  ['vi', 'Vietnamese'],
  ['he', 'Hebrew'],
  ['uk', 'Ukrainian'],
  ['el', 'Greek'],
  ['ms', 'Malay'],
  ['cs', 'Czech'],
  ['ro', 'Romanian'],
  ['da', 'Danish'],
  ['hu', 'Hungarian'],
  ['ta', 'Tamil'],
  ['no', 'Norwegian'],
  ['th', 'Thai'],
  ['ur', 'Urdu'],
  ['hr', 'Croatian'],
  ['bg', 'Bulgarian'],
  ['lt', 'Lithuanian'],
  ['la', 'Latin'],
  ['mi', 'Maori'],
  ['ml', 'Malayalam'],
  ['cy', 'Welsh'],
  ['sk', 'Slovak'],
  ['te', 'Telugu'],
  ['fa', 'Persian'],
  ['lv', 'Latvian'],
  ['bn', 'Bengali'],
  ['sr', 'Serbian'],
  ['az', 'Azerbaijani'],
  ['sl', 'Slovenian'],
  ['kn', 'Kannada'],
  ['et', 'Estonian'],
  ['mk', 'Macedonian'],
  ['br', 'Breton'],
  ['eu', 'Basque'],
  ['is', 'Icelandic'],
  ['hy', 'Armenian'],
  ['ne', 'Nepali'],
  ['mn', 'Mongolian'],
  ['bs', 'Bosnian'],
  ['kk', 'Kazakh'],
  ['sq', 'Albanian'],
  ['sw', 'Swahili'],
  ['gl', 'Galician'],
  ['mr', 'Marathi'],
  ['pa', 'Punjabi'],
  ['si', 'Sinhala'],
  ['km', 'Khmer'],
  ['sn', 'Shona'],
  ['yo', 'Yoruba'],
  ['so', 'Somali'],
  ['af', 'Afrikaans'],
  ['oc', 'Occitan'],
  ['ka', 'Georgian'],
  ['be', 'Belarusian'],
  ['tg', 'Tajik'],
  ['sd', 'Sindhi'],
  ['gu', 'Gujarati'],
  ['am', 'Amharic'],
  ['yi', 'Yiddish'],
  ['lo', 'Lao'],
  ['uz', 'Uzbek'],
  ['fo', 'Faroese'],
  ['ht', 'Haitian Creole'],
  ['ps', 'Pashto'],
  ['tk', 'Turkmen'],
  ['nn', 'Nynorsk'],
  ['mt', 'Maltese'],
  ['sa', 'Sanskrit'],
  ['lb', 'Luxembourgish'],
  ['my', 'Myanmar'],
  ['bo', 'Tibetan'],
  ['tl', 'Tagalog'],
  ['mg', 'Malagasy'],
  ['as', 'Assamese'],
  ['tt', 'Tatar'],
  ['haw', 'Hawaiian'],
  ['ln', 'Lingala'],
  ['ha', 'Hausa'],
  ['ba', 'Bashkir'],
  ['jw', 'Javanese'],
  ['su', 'Sundanese'],
] as const;

export type WhisperSourceLanguage = (typeof WHISPER_SOURCE_LANGUAGES)[number][0];

const sourceLanguageCodes = WHISPER_SOURCE_LANGUAGES.map(([code]) => code) as [
  WhisperSourceLanguage,
  ...WhisperSourceLanguage[],
];

export const WhisperSourceLanguageSchema = z.enum(sourceLanguageCodes);
export const WhisperLanguageSchema = z.union([
  z.literal(WHISPER_AUTO_LANGUAGE),
  WhisperSourceLanguageSchema,
]);
export type WhisperLanguage = z.infer<typeof WhisperLanguageSchema>;

const LEGACY_LANGUAGE_ALIASES = new Map<string, WhisperSourceLanguage>([
  ...WHISPER_SOURCE_LANGUAGES.flatMap(([code, name]) => [
    [code, code] as const,
    [name.toLowerCase(), code] as const,
  ]),
  ['burmese', 'my'],
  ['valencian', 'ca'],
  ['flemish', 'nl'],
  ['haitian', 'ht'],
  ['letzeburgesch', 'lb'],
  ['pushto', 'ps'],
  ['panjabi', 'pa'],
  ['moldavian', 'ro'],
  ['moldovan', 'ro'],
  ['sinhalese', 'si'],
  ['castilian', 'es'],
  // Common UI values that older free-form settings allowed, although the pinned
  // Transformers.js parser itself did not recognize them.
  ['mandarin', 'zh'],
]);

export function normalizeWhisperSourceLanguage(value: unknown): WhisperLanguage {
  if (typeof value !== 'string') return WHISPER_AUTO_LANGUAGE;
  const normalized = value.trim().toLowerCase();
  if (normalized === WHISPER_AUTO_LANGUAGE || normalized === 'auto-detect' || normalized === '') {
    return WHISPER_AUTO_LANGUAGE;
  }
  return LEGACY_LANGUAGE_ALIASES.get(normalized) ?? WHISPER_AUTO_LANGUAGE;
}
