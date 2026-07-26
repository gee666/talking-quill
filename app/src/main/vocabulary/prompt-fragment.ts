import type { VocabularyEntry } from '../../shared/schemas/vocabulary';
import { normalizeCommandText } from '../../shared/text/command-normalization';

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

function compareCodePoints(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}
