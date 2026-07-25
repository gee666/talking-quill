import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import { ModelManifestSchema } from '../../app/src/shared/schemas/model-manifest';

const required = [
  'config.json',
  'generation_config.json',
  'preprocessor_config.json',
  'tokenizer.json',
  'tokenizer_config.json',
  'onnx/encoder_model_quantized.onnx',
  'onnx/decoder_model_merged_quantized.onnx',
];

describe('pinned Whisper model manifest', () => {
  it('contains only the recommended turbo and compact small immutable q8 revisions', async () => {
    const manifest = ModelManifestSchema.parse(
      JSON.parse(await readFile('scripts/model-manifest.json', 'utf8')) as unknown,
    );
    expect(manifest.models.map((model) => [model.id, model.revision, model.totalBytes])).toEqual([
      [
        'onnx-community/whisper-large-v3-turbo',
        '360ebcde2559d60bb474678be3c1de9ef347d01a',
        1_087_527_940,
      ],
      ['Xenova/whisper-small', '2d67713f236afa48a18992566e7647f6ca848e13', 251_875_316],
    ]);
    for (const model of manifest.models) {
      expect(model.dtype).toBe('q8');
      expect(model.files.map((file) => file.path)).toEqual(required);
      expect(model.files.reduce((sum, file) => sum + file.size, 0)).toBe(model.totalBytes);
      expect(model.files.every((file) => /^[a-f0-9]{64}$/.test(file.sha256))).toBe(true);
    }
  });
});
