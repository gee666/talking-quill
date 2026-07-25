import {
  VOCABULARY_FILE_MAX_BYTES,
  VOCABULARY_LIMIT,
  VOCABULARY_TOTAL_MAX_UTF8_BYTES,
  VocabularyValueSchema,
} from '../../shared/schemas/vocabulary';
import { utf8ByteLength } from '../../shared/schemas/text-bounds';
import { normalizeCommandText } from '../commands/matcher';
import { PublicAppError } from '../security/public-error';

export function parseVocabularyText(bytes: Uint8Array): readonly string[] {
  if (bytes.byteLength > VOCABULARY_FILE_MAX_BYTES)
    invalid('The vocabulary file is larger than 1 MB.');
  let text: string;
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    invalid('The vocabulary file must be valid UTF-8 text.');
  }
  text = text.replace(/^\uFEFF/u, '').replace(/\r\n?/gu, '\n');
  if (text.includes(String.fromCodePoint(0))) invalid('The vocabulary file contains binary data.');
  const values = text
    .split('\n')
    .map((value) => value.trim())
    .filter((value) => value.length > 0)
    .map((value) => VocabularyValueSchema.parse(value));
  if (values.length > VOCABULARY_LIMIT) invalid('The vocabulary file contains too many entries.');
  if (
    values.reduce((total, value) => total + utf8ByteLength(value), 0) >
    VOCABULARY_TOTAL_MAX_UTF8_BYTES
  ) {
    invalid('The vocabulary file exceeds the total UTF-8 size limit.');
  }
  const seen = new Set<string>();
  for (const value of values) {
    const normalized = normalizeCommandText(value);
    if (seen.has(normalized))
      invalid(`The vocabulary file contains a duplicate entry: “${value}”.`);
    seen.add(normalized);
  }
  return values;
}

export function serializeVocabularyText(values: readonly string[]): string {
  const parsed = values.map((value) => VocabularyValueSchema.parse(value));
  if (
    parsed.reduce((total, value) => total + utf8ByteLength(value), 0) >
    VOCABULARY_TOTAL_MAX_UTF8_BYTES
  ) {
    invalid('Custom vocabulary exceeds the total UTF-8 size limit.');
  }
  return parsed.length === 0 ? '' : `${parsed.join('\n')}\n`;
}

function invalid(message: string): never {
  throw new PublicAppError({ code: 'BAD_REQUEST', message });
}
