import { randomUUID } from 'node:crypto';
import {
  VOCABULARY_LIMIT,
  VOCABULARY_TOTAL_MAX_UTF8_BYTES,
  VocabularyEntrySchema,
  VocabularyListSchema,
  VocabularyValueSchema,
  type VocabularyEntry,
} from '../../shared/schemas/vocabulary';
import { utf8ByteLength } from '../../shared/schemas/text-bounds';
import { normalizeCommandText } from '../../shared/text/command-normalization';
import type { SettingsStore } from '../persistence/settings-store';
import { PublicAppError } from '../security/public-error';

export class VocabularyStore {
  readonly #settings: SettingsStore;
  #mutationTail: Promise<void> = Promise.resolve();

  constructor(settings: SettingsStore) {
    this.#settings = settings;
  }

  list(): readonly VocabularyEntry[] {
    return VocabularyListSchema.parse(this.#settings.get().customVocabulary);
  }

  create(value: string): Promise<VocabularyEntry> {
    return this.#mutate(() => this.#append([value])).then((entries) => requireFirst(entries));
  }

  import(values: readonly string[]): Promise<readonly VocabularyEntry[]> {
    return this.#mutate(() => this.#append(values));
  }

  update(id: string, value: string): Promise<VocabularyEntry> {
    return this.#mutate(async () => {
      const parsed = VocabularyValueSchema.parse(value);
      const entries = [...this.list()];
      const index = entries.findIndex((entry) => entry.id === id);
      const current = entries[index];
      if (current === undefined) notFound();
      this.#assertUnique(parsed, entries, id);
      const updated = VocabularyEntrySchema.parse({
        ...current,
        value: parsed,
        updatedAt: Date.now(),
      });
      entries[index] = updated;
      await this.#settings.update({ customVocabulary: entries });
      return updated;
    });
  }

  delete(id: string): Promise<boolean> {
    return this.#mutate(async () => {
      const entries = [...this.list()];
      const remaining = entries.filter((entry) => entry.id !== id);
      if (remaining.length === entries.length) return false;
      await this.#settings.update({ customVocabulary: remaining });
      return true;
    });
  }

  async #append(values: readonly string[]): Promise<readonly VocabularyEntry[]> {
    const parsed = values.map((value) => VocabularyValueSchema.parse(value));
    const entries = [...this.list()];
    if (entries.length + parsed.length > VOCABULARY_LIMIT) {
      throw new PublicAppError({
        code: 'BAD_REQUEST',
        message: 'The custom vocabulary limit has been reached.',
      });
    }
    const totalBytes = [...entries.map((entry) => entry.value), ...parsed].reduce(
      (total, value) => total + utf8ByteLength(value),
      0,
    );
    if (totalBytes > VOCABULARY_TOTAL_MAX_UTF8_BYTES) {
      throw new PublicAppError({
        code: 'BAD_REQUEST',
        message: 'Custom vocabulary exceeds the total UTF-8 size limit.',
      });
    }
    const normalized = new Set(entries.map((entry) => normalizeCommandText(entry.value)));
    const pending: VocabularyEntry[] = [];
    for (const value of parsed) {
      const key = normalizeCommandText(value);
      if (normalized.has(key)) {
        throw new PublicAppError({
          code: 'BAD_REQUEST',
          message: `“${value}” is already in custom vocabulary.`,
        });
      }
      normalized.add(key);
      const now = Date.now();
      pending.push(
        VocabularyEntrySchema.parse({ id: randomUUID(), value, createdAt: now, updatedAt: now }),
      );
    }
    if (pending.length > 0)
      await this.#settings.update({ customVocabulary: [...entries, ...pending] });
    return pending;
  }

  #assertUnique(value: string, entries: readonly VocabularyEntry[], excludedId?: string): void {
    const normalized = normalizeCommandText(value);
    const conflict = entries.find(
      (entry) => entry.id !== excludedId && normalizeCommandText(entry.value) === normalized,
    );
    if (conflict !== undefined) {
      throw new PublicAppError({
        code: 'BAD_REQUEST',
        message: `“${value}” is already in custom vocabulary.`,
      });
    }
  }

  #mutate<Value>(operation: () => Promise<Value>): Promise<Value> {
    const result = this.#mutationTail.then(operation, operation);
    this.#mutationTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function requireFirst(entries: readonly VocabularyEntry[]): VocabularyEntry {
  const first = entries[0];
  if (first === undefined) throw new Error('Vocabulary entry was not created');
  return first;
}
function notFound(): never {
  throw new PublicAppError({
    code: 'NOT_FOUND',
    message: 'The vocabulary entry no longer exists.',
  });
}
