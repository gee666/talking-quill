import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const appRequire = createRequire(resolve('app', 'package.json'));

describe('@huggingface/transformers 3.8.1 Whisper timestamp compatibility', () => {
  it('uses supported segment timestamps instead of attention-dependent word timestamps', async () => {
    const entry = appRequire.resolve('@huggingface/transformers');
    const packageRoot = resolve(dirname(entry), '..');
    const [packageText, pipelineTypes, pipelineSource, modelSource] = await Promise.all([
      readFile(join(packageRoot, 'package.json'), 'utf8'),
      readFile(join(packageRoot, 'types', 'pipelines.d.ts'), 'utf8'),
      readFile(join(packageRoot, 'src', 'pipelines.js'), 'utf8'),
      readFile(join(packageRoot, 'src', 'models.js'), 'utf8'),
    ]);
    expect(JSON.parse(packageText)).toMatchObject({ version: '3.8.1' });
    expect(pipelineTypes).toContain('return_timestamps?: boolean | "word";');
    expect(pipelineTypes).toContain('timestamp: [number, number];');
    expect(pipelineSource).toContain("if (return_timestamps === 'word')");
    expect(pipelineSource).toContain("generation_config['return_token_timestamps'] = true;");
    expect(modelSource).toContain('if (!generate_outputs.cross_attentions)');
  });
});
