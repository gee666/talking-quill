import type { VocabularyEntry } from '../../shared/schemas/vocabulary';
import { compareCodePoints, normalizeCommandText } from '../commands/matcher';

export function buildVocabularyPromptFragment(entries: readonly VocabularyEntry[]): string {
  if (entries.length === 0) return '';
  const values = entries
    .map((entry) => entry.value)
    .sort(
      (left, right) =>
        compareCodePoints(normalizeCommandText(left), normalizeCommandText(right)) ||
        compareCodePoints(left, right),
    );
  return `\n\nCustom vocabulary (preserve these spellings when context supports them):\n${JSON.stringify(values)}`;
}
