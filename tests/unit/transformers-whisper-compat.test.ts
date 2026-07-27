import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { WHISPER_SOURCE_LANGUAGES } from '../../app/src/shared/schemas/whisper-languages';

const appRequire = createRequire(resolve('app', 'package.json'));

describe('@huggingface/transformers 3.8.1 Whisper compatibility', () => {
  it('pins timestamp, source-language, and transcribe-task semantics used by the runtime', async () => {
    const entry = appRequire.resolve('@huggingface/transformers');
    const packageRoot = resolve(dirname(entry), '..');
    const [packageText, pipelineTypes, pipelineSource, modelSource, languageSource] =
      await Promise.all([
        readFile(join(packageRoot, 'package.json'), 'utf8'),
        readFile(join(packageRoot, 'types', 'pipelines.d.ts'), 'utf8'),
        readFile(join(packageRoot, 'src', 'pipelines.js'), 'utf8'),
        readFile(join(packageRoot, 'src', 'models.js'), 'utf8'),
        readFile(join(packageRoot, 'src', 'models', 'whisper', 'common_whisper.js'), 'utf8'),
      ]);
    expect(JSON.parse(packageText)).toMatchObject({ version: '3.8.1' });
    expect(pipelineTypes).toContain('return_timestamps?: boolean | "word";');
    expect(pipelineTypes).toContain('timestamp: [number, number];');
    expect(pipelineSource).toContain("if (return_timestamps === 'word')");
    expect(pipelineSource).toContain("generation_config['return_token_timestamps'] = true;");
    expect(modelSource).toContain('if (!generate_outputs.cross_attentions)');
    expect(pipelineTypes).toContain('[language] The source language.');
    expect(pipelineTypes).toContain("{ language: 'french', task: 'transcribe' }");
    expect(pipelineTypes).toContain("{ language: 'french', task: 'translate' }");
    expect(modelSource).toContain("generation_config.task_to_id[task ?? 'transcribe']");
    // Auto mode must keep using our detector until the pinned library implements this itself.
    expect(modelSource).toContain('// TODO: Implement language detection');
    expect(modelSource).toContain("language = 'en';");
    expect(modelSource).toContain(
      'const init_tokens = kwargs.decoder_input_ids ?? this._retrieve_init_tokens(generation_config);',
    );
    for (const [code, name] of WHISPER_SOURCE_LANGUAGES) {
      expect(languageSource).toContain(`["${code}", "${name.toLowerCase()}"]`);
    }
  });
});
