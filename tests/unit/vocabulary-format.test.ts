import { describe, expect, it } from 'vitest';
import { buildVocabularyPromptFragment } from '../../app/src/main/vocabulary/prompt-fragment';
import {
  parseVocabularyText,
  serializeVocabularyText,
} from '../../app/src/main/vocabulary/text-format';
import {
  VOCABULARY_FILE_MAX_BYTES,
  type VocabularyEntry,
} from '../../app/src/shared/schemas/vocabulary';

const encoder = new TextEncoder();
const entry = (id: string, value: string): VocabularyEntry => ({
  id,
  value,
  createdAt: 1,
  updatedAt: 1,
});

describe('vocabulary text format and prompt fragment', () => {
  it('imports BOM and all common line endings and exports deterministic LF text', () => {
    const values = parseVocabularyText(encoder.encode('\uFEFFGraphQL\rJosé\r\nAnythingLLM\n'));
    expect(values).toEqual(['GraphQL', 'José', 'AnythingLLM']);
    expect(serializeVocabularyText(values)).toBe('GraphQL\nJosé\nAnythingLLM\n');
    expect(serializeVocabularyText([])).toBe('');
  });

  it.each([
    [new Uint8Array([0xff, 0xfe]), 'UTF-8'],
    [encoder.encode('valid\nnull\u0000byte'), 'binary'],
    [encoder.encode('GraphQL\ngraphql'), 'duplicate'],
    [new Uint8Array(VOCABULARY_FILE_MAX_BYTES + 1), 'larger'],
  ])('rejects invalid input atomically', (bytes, message) => {
    expect(() => parseVocabularyText(bytes)).toThrow(message);
  });

  it('builds a stable, escaped Task 10 fragment independent of input order', () => {
    const first = [entry('b', 'José'), entry('a', 'Anything "LLM"')];
    const second = [...first].reverse();
    expect(buildVocabularyPromptFragment(first)).toBe(buildVocabularyPromptFragment(second));
    expect(buildVocabularyPromptFragment(first)).toContain('["Anything \\"LLM\\"","José"]');
    expect(buildVocabularyPromptFragment([])).toBe('');
  });
});
