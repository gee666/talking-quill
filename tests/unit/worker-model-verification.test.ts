import { createHash } from 'node:crypto';
import { mkdir, rm, stat, utimes, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { verifyModelFiles } from '../../app/src/workers/whisper/verify-model';
import { ModelManifestEntrySchema } from '../../app/src/shared/schemas/model-manifest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

describe('worker-side model verification', () => {
  it('distinguishes missing files from same-size corruption with restored mtime', async () => {
    const root = await createTestDirectory('worker-model-verify');
    roots.push(root);
    const paths = [
      'config.json',
      'generation_config.json',
      'preprocessor_config.json',
      'tokenizer.json',
      'tokenizer_config.json',
      'onnx/encoder_model_quantized.onnx',
      'onnx/decoder_model_merged_quantized.onnx',
    ];
    const files = paths.map((path, index) => {
      const bytes = Buffer.from(`verified-${String(index)}`);
      return {
        path,
        bytes,
        size: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
      };
    });
    const model = ModelManifestEntrySchema.parse({
      id: 'Xenova/whisper-small',
      revision: 'a'.repeat(40),
      dtype: 'q8',
      totalBytes: files.reduce((sum, file) => sum + file.size, 0),
      files: files.map(({ path, size, sha256 }) => ({ path, size, sha256 })),
    });
    const modelRoot = join(root, 'Xenova', 'whisper-small', model.revision);
    for (const file of files) {
      const target = join(modelRoot, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, file.bytes);
    }
    await expect(verifyModelFiles(root, model)).resolves.toBeUndefined();
    const first = files[0];
    if (first === undefined) throw new Error('Fixture missing');
    const firstPath = join(modelRoot, first.path);
    const metadata = await stat(firstPath);
    const corrupt = Buffer.from(first.bytes);
    corrupt[0] = (corrupt[0] ?? 0) ^ 0xff;
    await writeFile(firstPath, corrupt);
    await utimes(firstPath, metadata.atime, metadata.mtime);
    await expect(verifyModelFiles(root, model)).rejects.toMatchObject({ code: 'MODEL_CORRUPT' });

    await rm(firstPath);
    await expect(verifyModelFiles(root, model)).rejects.toMatchObject({ code: 'MODEL_MISSING' });
  });
});
