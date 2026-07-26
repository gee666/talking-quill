import { describe, expect, it } from 'vitest';
import { normalizeModels } from '../../app/src/main/providers/openai-compatible-models';

describe('OpenAI-compatible model normalization', () => {
  it('expands Docker tags without copying unrelated source metadata per tag', () => {
    let unrelatedReads = 0;
    const source: Record<string, unknown> = {
      tags: Array.from({ length: 100 }, (_, index) => `namespace/model-${String(index)}`),
      config: { gguf: { 'model.context_length': 8_192 } },
    };
    Object.defineProperty(source, 'unrelatedMetadata', {
      enumerable: true,
      get: () => {
        unrelatedReads += 1;
        return 'unused';
      },
    });

    const models = normalizeModels([source], 'docker', 'all', 4_096, {}, 'unknown');

    expect(models).toHaveLength(100);
    expect(models[0]).toMatchObject({
      id: 'namespace/model-0',
      name: 'model-0',
      contextWindow: 8_192,
    });
    expect(unrelatedReads).toBe(0);
  });
});
