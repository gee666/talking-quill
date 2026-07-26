import { createHash } from 'node:crypto';
import { mkdir, readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { ModelRepository } from '../../app/src/main/transcription/model-repository';
import type { ModelManifestEntry } from '../../app/src/shared/schemas/model-manifest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const body = Buffer.from('persisted-model-data');
const revision = 'b'.repeat(40);
const model = {
  id: 'Xenova/whisper-small',
  revision,
  dtype: 'q8',
  totalBytes: body.byteLength,
  files: [
    {
      path: 'onnx/model.onnx',
      size: body.byteLength,
      sha256: createHash('sha256').update(body).digest('hex'),
    },
  ],
} as unknown as ModelManifestEntry;
const file = model.files[0];
if (file === undefined) throw new Error('Fixture missing');
const roots: string[] = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map(removeTestDirectory));
});

describe('ModelRepository persisted layout', () => {
  it('keeps part, staged revision, installed revision, and marker formats compatible', async () => {
    const root = await createTestDirectory('model-repository-layout');
    roots.push(root);
    const modelsDirectory = join(root, 'models');
    const temporaryDirectory = join(modelsDirectory, '.tmp');
    await mkdir(temporaryDirectory, { recursive: true });
    const repository = new ModelRepository({ modelsDirectory, temporaryDirectory });
    await repository.prepareRoots();
    await repository.recoverArtifacts(model);

    const writer = await repository.openPartialWriter(model, file, 0);
    await writer.write(body);
    await writer.sync();
    await writer.close();

    const part = join(
      temporaryDirectory,
      'Xenova',
      'whisper-small',
      revision,
      'onnx',
      'model.onnx.part',
    );
    expect(await readFile(part)).toEqual(body);
    await repository.publishVerifiedPartial(model, file, null, new AbortController().signal);

    const staged = join(
      temporaryDirectory,
      'Xenova',
      'whisper-small',
      revision,
      'onnx',
      'model.onnx',
    );
    expect(await readFile(staged)).toEqual(body);
    await expect(readFile(part)).rejects.toMatchObject({ code: 'ENOENT' });
    const stagedInspection = await repository.inspectStaging(model);
    expect(stagedInspection).toMatchObject({ valid: true, validBytes: body.byteLength });
    expect(await repository.temporaryBytes(model)).toBe(body.byteLength);

    await repository.prepareStagedPublication(model, stagedInspection.identities);
    await repository.publishStagedRevision(model);
    await repository.assertPublishedManifestEntries(model);
    const installedInspection = await repository.inspectInstalled(model);
    expect(installedInspection.valid).toBe(true);
    await repository.commitVerification(model, installedInspection.identities);

    const installedDirectory = join(modelsDirectory, 'Xenova', 'whisper-small', revision);
    const markerPath = join(installedDirectory, '.talking-quill-complete.json');
    expect(await readFile(join(installedDirectory, 'onnx', 'model.onnx'))).toEqual(body);
    const markerText = await readFile(markerPath, 'utf8');
    expect(markerText.endsWith('\n')).toBe(true);
    const marker = JSON.parse(markerText) as Record<string, unknown>;
    expect(marker).toMatchObject({
      schemaVersion: 1,
      revision,
      totalBytes: body.byteLength,
      files: [{ path: file.path, size: file.size }],
    });
    await expect(stat(staged)).rejects.toMatchObject({ code: 'ENOENT' });

    const restarted = new ModelRepository({ modelsDirectory, temporaryDirectory });
    const completion = await restarted.readCompletionMarker(model);
    expect(completion.present).toBe(true);
    expect(completion.identity).toHaveLength(1);
    expect(
      completion.identity !== null &&
        (await restarted.installedIdentityStillCurrent(model, completion.identity)),
    ).toBe(true);
  });
});
