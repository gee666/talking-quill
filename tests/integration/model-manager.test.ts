import { createHash } from 'node:crypto';
import {
  link,
  mkdir,
  readFile,
  rename,
  rm,
  stat,
  symlink,
  utimes,
  writeFile,
} from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { inspectFile } from '../../app/src/main/transcription/model-integrity';
import {
  ModelAccessCoordinator,
  type ModelAccessLease,
} from '../../app/src/main/transcription/model-access-coordinator';
import { ModelManager } from '../../app/src/main/transcription/model-manager';
import {
  WhisperClientError,
  type ModelManagerError,
} from '../../app/src/main/transcription/errors';
import {
  ModelManifestSchema,
  type ModelManifest,
  type ModelManifestFile,
} from '../../app/src/shared/schemas/model-manifest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';
import { startRangeServer } from '../helpers/range-server';

const paths = [
  'config.json',
  'generation_config.json',
  'preprocessor_config.json',
  'tokenizer.json',
  'tokenizer_config.json',
  'onnx/encoder_model_quantized.onnx',
  'onnx/decoder_model_merged_quantized.onnx',
];
const bodies = new Map(
  paths.map((path, index) => [path, Buffer.from(`${path}:${'x'.repeat(index + 8)}`)]),
);
const files: ModelManifestFile[] = paths.map((path) => {
  const body = bodies.get(path);
  if (body === undefined) throw new Error('Fixture missing');
  return { path, size: body.byteLength, sha256: createHash('sha256').update(body).digest('hex') };
});
const totalBytes = files.reduce((sum, file) => sum + file.size, 0);
const smallRevision = 'a'.repeat(40);
const manifest = ModelManifestSchema.parse({
  schemaVersion: 1,
  transformersVersion: '3.8.1',
  models: [
    {
      id: 'onnx-community/whisper-large-v3-turbo',
      revision: 'c'.repeat(40),
      dtype: 'q8',
      totalBytes,
      files,
    },
    { id: 'Xenova/whisper-small', revision: smallRevision, dtype: 'q8', totalBytes, files },
  ],
});
const roots: string[] = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map(removeTestDirectory));
});

describe('ModelManager', () => {
  it('resumes validated ranges, reports bounded progress, writes a marker, detects corruption, and deletes', async () => {
    const root = await createTestDirectory('model-manager');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const first = files[0];
      if (first === undefined) throw new Error('Fixture missing');
      const firstBody = bodies.get(first.path);
      if (firstBody === undefined) throw new Error('Fixture missing');
      const part = join(
        root,
        'models',
        '.tmp',
        'Xenova',
        'whisper-small',
        smallRevision,
        `${first.path}.part`,
      );
      await mkdir(join(part, '..'), { recursive: true });
      await writeFile(part, firstBody.subarray(0, 5));
      const progress: number[] = [];
      const states: string[] = [];
      const fetchModel = vi.fn<typeof fetch>((input, init) => fetch(input, init));
      const manager = createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
        fetchModel,
      );
      manager.subscribe((event) => {
        progress.push(event.total.downloadedBytes);
        states.push(event.state);
      });
      const complete = await manager.download('Xenova/whisper-small');
      expect(complete.state).toBe('ready');
      expect(server.ranges).toContain('bytes=5-');
      expect(progress.every((value) => value >= 0 && value <= totalBytes)).toBe(true);
      expect(progress.at(-1)).toBe(totalBytes);
      expect(states.slice(-3)).toEqual(['verifying', 'installing', 'ready']);
      const marker = await readFile(
        join(
          root,
          'models',
          'Xenova',
          'whisper-small',
          smallRevision,
          '.talking-quill-complete.json',
        ),
        'utf8',
      );
      expect(marker).toContain(smallRevision);

      const corruptPath = join(
        root,
        'models',
        'Xenova',
        'whisper-small',
        smallRevision,
        first.path,
      );
      const originalMetadata = await stat(corruptPath);
      const corrupted = Buffer.from(firstBody);
      corrupted[0] = (corrupted[0] ?? 0) ^ 0xff;
      await writeFile(corruptPath, corrupted);
      await utimes(corruptPath, originalMetadata.atime, originalMetadata.mtime);
      expect((await manager.status('Xenova/whisper-small')).state).toBe('corrupt');
      expect((await manager.verifyForUse('Xenova/whisper-small')).state).toBe('corrupt');
      fetchModel.mockClear();
      expect((await manager.retry('Xenova/whisper-small')).state).toBe('ready');
      expect(fetchModel).toHaveBeenCalledOnce();
      expect((await manager.delete('Xenova/whisper-small')).state).toBe('missing');
      await manager.shutdown();
    } finally {
      await server.close();
    }
  });

  it('falls back to verified downloads when repair hard links are unavailable', async () => {
    const root = await createTestDirectory('model-repair-link-fallback');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    const urlFor = (_model: unknown, file: ModelManifestFile) =>
      `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`;
    try {
      await createManager(root, (file) => urlFor(undefined, file)).download('Xenova/whisper-small');
      const first = files[0];
      if (first === undefined) throw new Error('Fixture missing');
      const installedFirst = join(
        root,
        'models',
        'Xenova',
        'whisper-small',
        smallRevision,
        first.path,
      );
      await writeFile(installedFirst, Buffer.alloc(first.size, 0x78));

      let fetches = 0;
      const unavailableLink = vi.fn<typeof link>(() =>
        Promise.reject(
          Object.assign(new Error('hard links disabled by policy'), { code: 'EPERM' }),
        ),
      );
      const fetchModel: typeof fetch = async (input, init) => {
        fetches += 1;
        return fetch(input, init);
      };
      const lowSpace = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        availableBytes: () => Promise.resolve(0),
        urlFor,
        validateRequestUrl: (url) => url.startsWith(server.origin),
        fetch: fetchModel,
        link: unavailableLink,
      });
      await expect(lowSpace.retry('Xenova/whisper-small')).rejects.toMatchObject({
        code: 'DISK_SPACE',
      });
      expect(fetches).toBe(0);
      expect(unavailableLink).toHaveBeenCalledTimes(files.length - 1);

      const repair = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
        urlFor,
        validateRequestUrl: (url) => url.startsWith(server.origin),
        fetch: fetchModel,
        link: unavailableLink,
      });
      await expect(repair.retry('Xenova/whisper-small')).resolves.toMatchObject({ state: 'ready' });
      expect(fetches).toBe(files.length);
      await expect(repair.verifyForUse('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
    } finally {
      await server.close();
    }
  });

  it('recognizes hard links left by an interrupted repair before publication', async () => {
    const root = await createTestDirectory('model-repair-staged-hard-links');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const urlFor = (file: ModelManifestFile) =>
        `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`;
      await createManager(root, urlFor).download('Xenova/whisper-small');
      const first = files[0];
      if (first === undefined) throw new Error('Fixture missing');
      const installedDirectory = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
      const stagedDirectory = join(
        root,
        'models',
        '.tmp',
        'Xenova',
        'whisper-small',
        smallRevision,
      );
      await writeFile(join(installedDirectory, first.path), Buffer.alloc(first.size, 0x78));
      for (const file of files.slice(1)) {
        const staged = join(stagedDirectory, ...file.path.split('/'));
        await mkdir(join(staged, '..'), { recursive: true });
        await link(join(installedDirectory, ...file.path.split('/')), staged);
      }

      let fetches = 0;
      const repair = createManager(root, urlFor, async (input, init) => {
        fetches += 1;
        return fetch(input, init);
      });
      await expect(repair.retry('Xenova/whisper-small')).resolves.toMatchObject({ state: 'ready' });
      expect(fetches).toBe(1);
      await expect(repair.verifyForUse('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
    } finally {
      await server.close();
    }
  });

  it('does not commit repaired identities before replacement backup cleanup succeeds', async () => {
    const root = await createTestDirectory('model-repair-backup-cleanup');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const urlFor = (_model: unknown, file: ModelManifestFile) =>
        `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`;
      await createManager(root, (file) => urlFor(undefined, file)).download('Xenova/whisper-small');
      const first = files[0];
      if (first === undefined) throw new Error('Fixture missing');
      const installedDirectory = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
      await writeFile(join(installedDirectory, first.path), Buffer.alloc(first.size, 0x78));

      let cleanupAttempts = 0;
      let fetches = 0;
      const repair = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
        urlFor,
        validateRequestUrl: (url) => url.startsWith(server.origin),
        fetch: async (input, init) => {
          fetches += 1;
          return fetch(input, init);
        },
        removeRevisionBackup: async (path) => {
          cleanupAttempts += 1;
          if (cleanupAttempts <= 2) {
            throw Object.assign(new Error('replacement backup is locked'), { code: 'EBUSY' });
          }
          await rm(path, { recursive: true, force: true });
        },
      });
      await expect(repair.retry('Xenova/whisper-small')).rejects.toBeInstanceOf(Error);
      await expect(
        readFile(join(installedDirectory, '.talking-quill-complete.json')),
      ).rejects.toMatchObject({ code: 'ENOENT' });
      expect(cleanupAttempts).toBe(2);
      expect(fetches).toBe(1);

      await expect(repair.verifyForUse('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
      expect(cleanupAttempts).toBe(3);
      expect(fetches).toBe(1);
      await expect(repair.status('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
    } finally {
      await server.close();
    }
  });

  it('isolates progress listener failures from installation and other observers', async () => {
    const root = await createTestDirectory('model-progress-listener');
    roots.push(root);
    const observed: string[] = [];
    const manager = createManager(
      root,
      (file) =>
        `https://trusted.example/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      (input) => {
        const value =
          input instanceof URL ? input.href : input instanceof Request ? input.url : input;
        const index = Number(new URL(value).pathname.slice(1));
        const file = files[index];
        const body = file === undefined ? undefined : bodies.get(file.path);
        return Promise.resolve(
          body === undefined ? new Response(null, { status: 404 }) : new Response(body),
        );
      },
    );
    manager.subscribe(() => {
      throw new Error('observer failed');
    });
    manager.subscribe((event) => observed.push(event.state));

    await expect(manager.download('Xenova/whisper-small')).resolves.toMatchObject({
      state: 'ready',
    });
    expect(observed).toContain('ready');
  });

  it('cancels download waits promptly without releasing an active model-use lease', async () => {
    const root = await createTestDirectory('model-download-lock-cancel');
    roots.push(root);
    const access = new ModelAccessCoordinator();
    const fetchModel = vi.fn<typeof fetch>();
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      accessCoordinator: access,
      fetch: fetchModel,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: () => 'https://trusted.example/model',
      validateRequestUrl: () => true,
    });
    await manager.initialize();
    const use = await access.acquireUse('Xenova/whisper-small');
    try {
      const external = new AbortController();
      const externallyCancelled = manager.download('Xenova/whisper-small', external.signal);
      external.abort();
      await expect(externallyCancelled).rejects.toMatchObject({ code: 'CANCELLED' });

      const primary = manager.download('Xenova/whisper-small');
      const followerController = new AbortController();
      const follower = manager.download('Xenova/whisper-small', followerController.signal);
      followerController.abort();
      await expect(follower).rejects.toMatchObject({ code: 'CANCELLED' });
      await expect(manager.pause('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'paused',
      });
      await expect(primary).resolves.toMatchObject({ state: 'paused' });

      const shuttingDown = manager.download('Xenova/whisper-small');
      await manager.shutdown();
      await expect(shuttingDown).resolves.toMatchObject({ state: 'missing' });
      expect(fetchModel).not.toHaveBeenCalled();
    } finally {
      use.release();
    }
  });

  it('aborts and awaits shared verification work during shutdown', async () => {
    const root = await createTestDirectory('model-verification-shutdown');
    roots.push(root);
    let markStarted!: () => void;
    const started = new Promise<void>((resolveStarted) => {
      markStarted = resolveStarted;
    });
    const inspect = vi.fn<typeof inspectFile>(
      (_path, _size, _sha256, _hash, signal) =>
        new Promise((_resolve, reject) => {
          markStarted();
          signal?.addEventListener(
            'abort',
            () => reject(new DOMException('Verification cancelled', 'AbortError')),
            { once: true },
          );
        }),
    );
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      inspectFile: inspect,
    });
    await manager.initialize();
    const verification = manager.verifyForUse('Xenova/whisper-small');
    await started;
    await manager.shutdown();
    await expect(verification).rejects.toMatchObject({ code: 'CANCELLED' });
    await expect(manager.verifyForUse('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CANCELLED',
    });
  });

  it('reclaims obsolete immutable installed and staged revisions during recovery', async () => {
    const root = await createTestDirectory('model-obsolete-revisions');
    roots.push(root);
    const obsoleteRevision = 'b'.repeat(40);
    const installed = join(root, 'models', 'Xenova', 'whisper-small', obsoleteRevision);
    const staged = join(root, 'models', '.tmp', 'Xenova', 'whisper-small', obsoleteRevision);
    await Promise.all([mkdir(installed, { recursive: true }), mkdir(staged, { recursive: true })]);
    await Promise.all([
      writeFile(join(installed, 'obsolete.bin'), 'obsolete'),
      writeFile(join(staged, 'obsolete.part'), 'obsolete'),
    ]);
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
    });
    await manager.initialize();
    await expect(stat(installed)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(stat(staged)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('can retry recovery after a transient filesystem failure', async () => {
    const root = await createTestDirectory('model-recovery-retry');
    roots.push(root);
    const modelsDirectory = join(root, 'models');
    await writeFile(modelsDirectory, 'temporarily blocks directory creation');
    const manager = new ModelManager({
      modelsDirectory,
      temporaryDirectory: join(root, 'temporary-models'),
      manifest,
    });
    await expect(manager.initialize()).rejects.toThrow();
    await rm(modelsDirectory);
    await expect(manager.initialize()).resolves.toBeUndefined();
  });

  it('restarts a ranged file safely when the server ignores Range with HTTP 200', async () => {
    const root = await createTestDirectory('model-range-ignored');
    roots.push(root);
    const first = files[0];
    if (first === undefined) throw new Error('Fixture missing');
    const firstBody = bodies.get(first.path);
    if (firstBody === undefined) throw new Error('Fixture missing');
    const part = join(
      root,
      'models',
      '.tmp',
      'Xenova',
      'whisper-small',
      smallRevision,
      `${first.path}.part`,
    );
    await mkdir(join(part, '..'), { recursive: true });
    await writeFile(part, firstBody.subarray(0, 5));
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
        ignoreRange: index === 0,
      })),
    );
    try {
      const manager = createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      );
      await expect(manager.download('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
      expect(server.ranges).toContain('bytes=5-');
      expect(
        await readFile(join(root, 'models', 'Xenova', 'whisper-small', smallRevision, first.path)),
      ).toEqual(firstBody);
    } finally {
      await server.close();
    }
  });

  it('accepts a valid complete 416 when a resumable part completes concurrently', async () => {
    const root = await createTestDirectory('model-range-416');
    roots.push(root);
    const first = files[0];
    if (first === undefined) throw new Error('Fixture missing');
    const firstBody = bodies.get(first.path);
    if (firstBody === undefined) throw new Error('Fixture missing');
    const part = join(
      root,
      'models',
      '.tmp',
      'Xenova',
      'whisper-small',
      smallRevision,
      `${first.path}.part`,
    );
    await mkdir(join(part, '..'), { recursive: true });
    await writeFile(part, firstBody.subarray(0, 5));
    const encodings: (string | null)[] = [];
    const fetch416: typeof fetch = async (input, init) => {
      encodings.push(new Headers(init?.headers).get('accept-encoding'));
      const url = new URL(input instanceof Request ? input.url : String(input));
      const index = Number(url.pathname.slice(1));
      const file = files[index];
      if (file === undefined) return new Response(null, { status: 404 });
      const body = bodies.get(file.path);
      if (body === undefined) throw new Error('Fixture missing');
      if (index === 0 && init?.headers !== undefined) {
        await writeFile(part, body);
        return new Response(null, {
          status: 416,
          headers: { 'content-range': `bytes */${String(body.byteLength)}` },
        });
      }
      return new Response(body, { status: 200 });
    };
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      fetch: fetch416,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: (_model, file) =>
        `https://trusted.example/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      validateRequestUrl: () => true,
    });
    await expect(manager.download('Xenova/whisper-small')).resolves.toMatchObject({
      state: 'ready',
    });
    expect(
      await readFile(join(root, 'models', 'Xenova', 'whisper-small', smallRevision, first.path)),
    ).toEqual(firstBody);
    expect(encodings.every((value) => value === 'identity')).toBe(true);
  });

  it('keeps completed models usable offline and fails before fetch when disk headroom is insufficient', async () => {
    const root = await createTestDirectory('model-offline');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    const manager = createManager(
      root,
      (file) =>
        `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
    );
    await manager.download('Xenova/whisper-small');
    await server.close();
    expect((await manager.status('Xenova/whisper-small')).state).toBe('ready');

    const emptyRoot = await createTestDirectory('model-no-space');
    roots.push(emptyRoot);
    let fetched = false;
    const noSpace = new ModelManager({
      modelsDirectory: join(emptyRoot, 'models'),
      temporaryDirectory: join(emptyRoot, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(0),
      fetch: () => {
        fetched = true;
        return Promise.reject(new Error('must not fetch'));
      },
    });
    await mkdir(join(emptyRoot, 'models', '.tmp'), { recursive: true });
    await expect(noSpace.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'DISK_SPACE',
    } satisfies Partial<ModelManagerError>);
    expect(fetched).toBe(false);
  });

  it('treats cancel on a ready model as a no-op', async () => {
    const root = await createTestDirectory('model-cancel-ready');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const manager = createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      );
      await manager.download('Xenova/whisper-small');
      const target = join(
        root,
        'models',
        'Xenova',
        'whisper-small',
        smallRevision,
        files[0]?.path ?? '',
      );
      const before = await readFile(target);
      await expect(manager.cancel('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
      expect(await readFile(target)).toEqual(before);
      expect((await manager.status('Xenova/whisper-small')).state).toBe('ready');

      manager.setBeforeMutation(() => Promise.reject(new Error('worker unload failed')));
      await expect(manager.delete('Xenova/whisper-small')).rejects.toThrow('worker unload failed');
      expect((await manager.status('Xenova/whisper-small')).state).toBe('ready');
      expect(await readFile(target)).toEqual(before);
    } finally {
      await server.close();
    }
  });

  it('fully verifies and marks a valid unmarked preseeded cache during acquireUse', async () => {
    const root = await createTestDirectory('model-preseeded');
    roots.push(root);
    const directory = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
    for (const file of files) {
      const target = join(directory, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, bodies.get(file.path) ?? new Uint8Array());
    }
    let hashes = 0;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      inspectFile: (...arguments_) => {
        if (arguments_[3]) hashes += 1;
        return inspectFile(...arguments_);
      },
    });
    const grant = await manager.acquireUse('Xenova/whisper-small');
    expect(grant.status.state).toBe('ready');
    expect(hashes).toBe(7);
    grant.release();
    expect(await readFile(join(directory, '.talking-quill-complete.json'), 'utf8')).toContain(
      smallRevision,
    );
  });

  it('hashes a complete staged revision only once per file before atomic publication', async () => {
    const root = await createTestDirectory('model-complete-staging');
    roots.push(root);
    const directory = join(root, 'models', '.tmp', 'Xenova', 'whisper-small', smallRevision);
    for (const file of files) {
      const target = join(directory, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, bodies.get(file.path) ?? new Uint8Array());
    }
    const first = files[0];
    if (first === undefined) throw new Error('Fixture missing');
    await writeFile(join(directory, `${first.path}.part`), 'stale partial data');
    let hashes = 0;
    let fetched = false;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(0),
      fetch: () => {
        fetched = true;
        return Promise.reject(new Error('complete staging must not fetch'));
      },
      inspectFile: async (...arguments_) => {
        const result = await inspectFile(...arguments_);
        if (arguments_[3] && result.exists) hashes += 1;
        return result;
      },
    });

    await expect(manager.download('Xenova/whisper-small')).resolves.toMatchObject({
      state: 'ready',
    });
    expect(hashes).toBe(files.length);
    expect(fetched).toBe(false);
    await expect(
      readFile(
        join(root, 'models', 'Xenova', 'whisper-small', smallRevision, `${first.path}.part`),
      ),
    ).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('rejects unexpected entries in complete staging before directory publication', async () => {
    const root = await createTestDirectory('model-complete-staging-extra');
    roots.push(root);
    const stagedDirectory = join(root, 'models', '.tmp', 'Xenova', 'whisper-small', smallRevision);
    for (const file of files) {
      const target = join(stagedDirectory, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, bodies.get(file.path) ?? new Uint8Array());
    }
    await writeFile(join(stagedDirectory, 'unexpected.bin'), 'not in the manifest');
    let fetched = false;
    let published = false;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(0),
      fetch: () => {
        fetched = true;
        return Promise.reject(new Error('complete staging must not fetch'));
      },
      rename: async (source, destination) => {
        if (source === stagedDirectory) published = true;
        await rename(source, destination);
      },
    });

    await expect(manager.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CORRUPT',
      repairable: true,
    });
    expect(fetched).toBe(false);
    expect(published).toBe(false);
    await expect(
      readFile(join(root, 'models', 'Xenova', 'whisper-small', smallRevision, 'unexpected.bin')),
    ).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('fails closed when an unexpected staged entry appears during directory publication', async () => {
    const root = await createTestDirectory('model-publication-extra-race');
    roots.push(root);
    const stagedDirectory = join(root, 'models', '.tmp', 'Xenova', 'whisper-small', smallRevision);
    for (const file of files) {
      const target = join(stagedDirectory, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, bodies.get(file.path) ?? new Uint8Array());
    }
    let fetched = false;
    let changed = false;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(0),
      fetch: () => {
        fetched = true;
        return Promise.reject(new Error('complete staging must not fetch'));
      },
      rename: async (source, destination) => {
        if (source === stagedDirectory && !changed) {
          changed = true;
          await writeFile(join(stagedDirectory, 'unexpected.bin'), 'publication race');
        }
        await rename(source, destination);
      },
    });

    await expect(manager.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CORRUPT',
      repairable: true,
    });
    expect(fetched).toBe(false);
    await expect(
      readFile(join(root, 'models', 'Xenova', 'whisper-small', smallRevision, 'unexpected.bin')),
    ).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('fails closed when verified staging changes during directory publication', async () => {
    const root = await createTestDirectory('model-publication-identity-race');
    roots.push(root);
    const stagedDirectory = join(root, 'models', '.tmp', 'Xenova', 'whisper-small', smallRevision);
    for (const file of files) {
      const target = join(stagedDirectory, ...file.path.split('/'));
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, bodies.get(file.path) ?? new Uint8Array());
    }
    const first = files[0];
    if (first === undefined) throw new Error('Fixture missing');
    let changed = false;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(0),
      rename: async (source, destination) => {
        if (source === stagedDirectory && !changed) {
          changed = true;
          await writeFile(join(stagedDirectory, first.path), Buffer.alloc(first.size, 0x78));
        }
        await rename(source, destination);
      },
    });

    await expect(manager.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CORRUPT',
    });
    await expect(
      readFile(
        join(
          root,
          'models',
          'Xenova',
          'whisper-small',
          smallRevision,
          '.talking-quill-complete.json',
        ),
      ),
    ).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('accepts completion markers for manifests with a non-production file count', async () => {
    const root = await createTestDirectory('model-dynamic-marker-count');
    roots.push(root);
    const file = files[0];
    if (file === undefined) throw new Error('Fixture missing');
    const singleFileManifest = {
      schemaVersion: 1,
      transformersVersion: '3.8.1',
      models: [
        {
          id: 'Xenova/whisper-small',
          revision: smallRevision,
          dtype: 'q8',
          totalBytes: file.size,
          files: [file],
        },
      ],
    } as unknown as ModelManifest;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest: singleFileManifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      fetch: () => Promise.resolve(new Response(bodies.get(file.path))),
      validateRequestUrl: () => true,
    });
    await manager.download('Xenova/whisper-small');

    const restarted = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest: singleFileManifest,
    });
    await expect(restarted.status('Xenova/whisper-small')).resolves.toMatchObject({
      state: 'ready',
    });
  });

  it('keeps normal readiness metadata-only while explicit verification checksums', async () => {
    const root = await createTestDirectory('model-verification-cache');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      await createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      ).download('Xenova/whisper-small');
      let hashes = 0;
      const countedInspect: typeof inspectFile = (...arguments_) => {
        if (arguments_[3]) hashes += 1;
        return inspectFile(...arguments_);
      };
      const metadataManager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        inspectFile: countedInspect,
      });
      expect((await metadataManager.status('Xenova/whisper-small', false)).state).toBe('ready');
      await metadataManager.list(false);
      expect(hashes).toBe(0);
      expect((await metadataManager.status('Xenova/whisper-small', true)).state).toBe('ready');
      expect(hashes).toBe(7);

      hashes = 0;
      const freshManager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        inspectFile: countedInspect,
      });
      const firstUse = await freshManager.acquireUse('Xenova/whisper-small');
      expect(firstUse.status.state).toBe('ready');
      firstUse.release();
      expect(hashes).toBe(0);
      const secondUse = await freshManager.acquireUse('Xenova/whisper-small');
      secondUse.release();
      expect(hashes).toBe(0);
    } finally {
      await server.close();
    }
  });

  it('isolates cancellation for either waiter on a shared explicit verification', async () => {
    const root = await createTestDirectory('model-verification-waiters');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      await createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      ).download('Xenova/whisper-small');
      for (const cancelledWaiter of ['first', 'second'] as const) {
        let hashCalls = 0;
        let markStarted: () => void = () => undefined;
        const started = new Promise<void>((resolveStarted) => {
          markStarted = resolveStarted;
        });
        let releaseHash: () => void = () => undefined;
        const hashGate = new Promise<void>((resolveHash) => {
          releaseHash = resolveHash;
        });
        const delayedInspect: typeof inspectFile = async (...arguments_) => {
          if (arguments_[3]) {
            hashCalls += 1;
            if (hashCalls === 1) {
              markStarted();
              await hashGate;
            }
          }
          return inspectFile(...arguments_);
        };
        const manager = new ModelManager({
          modelsDirectory: join(root, 'models'),
          temporaryDirectory: join(root, 'models', '.tmp'),
          manifest,
          inspectFile: delayedInspect,
        });
        const firstController = new AbortController();
        const secondController = new AbortController();
        const first = manager.verifyForUse('Xenova/whisper-small', firstController.signal);
        const second = manager.verifyForUse('Xenova/whisper-small', secondController.signal);
        await started;
        await new Promise((resolveTurn) => setImmediate(resolveTurn));
        const cancelled = cancelledWaiter === 'first' ? first : second;
        const survivor = cancelledWaiter === 'first' ? second : first;
        if (cancelledWaiter === 'first') firstController.abort();
        else secondController.abort();
        await expect(cancelled).rejects.toMatchObject({ code: 'CANCELLED' });
        releaseHash();
        await expect(survivor).resolves.toMatchObject({ state: 'ready' });
        expect(hashCalls).toBe(7);
      }

      let internalAborted = false;
      let markCancellationStarted: () => void = () => undefined;
      const cancellationStarted = new Promise<void>((resolveStarted) => {
        markCancellationStarted = resolveStarted;
      });
      const cancellableInspect: typeof inspectFile = (...arguments_) => {
        if (!arguments_[3]) return inspectFile(...arguments_);
        markCancellationStarted();
        const signal = arguments_[4];
        return new Promise((_resolve, reject) => {
          signal?.addEventListener(
            'abort',
            () => {
              internalAborted = true;
              reject(new DOMException('cancelled', 'AbortError'));
            },
            { once: true },
          );
        });
      };
      const cancellationManager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        inspectFile: cancellableInspect,
      });
      const firstController = new AbortController();
      const secondController = new AbortController();
      const first = cancellationManager.verifyForUse(
        'Xenova/whisper-small',
        firstController.signal,
      );
      const second = cancellationManager.verifyForUse(
        'Xenova/whisper-small',
        secondController.signal,
      );
      await cancellationStarted;
      await new Promise((resolveTurn) => setImmediate(resolveTurn));
      firstController.abort();
      await expect(first).rejects.toMatchObject({ code: 'CANCELLED' });
      expect(internalAborted).toBe(false);
      secondController.abort();
      await expect(second).rejects.toMatchObject({ code: 'CANCELLED' });
      await waitForCondition(() => internalAborted);
    } finally {
      await server.close();
    }
  });

  it('lets queued deletion finish before the shared verification task obtains its sole use lease', async () => {
    const root = await createTestDirectory('model-verification-mutation-order');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      await createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      ).download('Xenova/whisper-small');
      const coordinator = new DelayedUseCoordinator();
      const manager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        accessCoordinator: coordinator,
      });
      await manager.initialize();
      coordinator.resetEvents();

      let allowMutation: () => void = () => undefined;
      const mutationBarrier = new Promise<void>((resolveMutation) => {
        allowMutation = resolveMutation;
      });
      let mutationEntered = false;
      manager.setBeforeMutation(async () => {
        mutationEntered = true;
        await mutationBarrier;
      });

      const verification = manager.status('Xenova/whisper-small', true);
      await coordinator.useRequested;
      expect(coordinator.useAcquireCalls).toBe(1);
      const deletion = manager.delete('Xenova/whisper-small');
      await waitForCondition(() => mutationEntered);
      coordinator.allowUseAcquisition();
      await new Promise((resolveTurn) => setImmediate(resolveTurn));
      expect(coordinator.events).not.toContain('use-granted');
      allowMutation();

      await expect(deletion).resolves.toMatchObject({ state: 'missing' });
      await expect(verification).resolves.toMatchObject({ state: 'missing' });
      expect(coordinator.events).toEqual(['mutation-granted', 'mutation-released', 'use-granted']);
      expect(coordinator.useAcquireCalls).toBe(1);
    } finally {
      await server.close();
    }
  });

  it('cancels a verification waiter promptly while deletion proceeds before task acquisition', async () => {
    const root = await createTestDirectory('model-verification-cancel-mutation');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      await createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      ).download('Xenova/whisper-small');
      const coordinator = new DelayedUseCoordinator();
      const manager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        accessCoordinator: coordinator,
      });
      await manager.initialize();
      coordinator.resetEvents();

      const controller = new AbortController();
      const verification = manager.verifyForUse('Xenova/whisper-small', controller.signal);
      await coordinator.useRequested;
      controller.abort();
      await expect(verification).rejects.toMatchObject({ code: 'CANCELLED' });
      const deletion = manager.delete('Xenova/whisper-small');
      await expect(deletion).resolves.toMatchObject({ state: 'missing' });
      coordinator.allowUseAcquisition();
      await waitForCondition(() => coordinator.internalUseCancelled);
      expect(coordinator.events).toEqual(['mutation-granted', 'mutation-released']);
      expect(coordinator.useAcquireCalls).toBe(1);
    } finally {
      await server.close();
    }
  });

  it('holds mutations behind active use and blocks new use between unload and delete', async () => {
    const root = await createTestDirectory('model-use-mutation-race');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const manager = createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      );
      await manager.download('Xenova/whisper-small');
      let mutationEntered = false;
      let allowMutation: () => void = () => undefined;
      const mutationBarrier = new Promise<void>((resolveBarrier) => {
        allowMutation = resolveBarrier;
      });
      manager.setBeforeMutation(async () => {
        mutationEntered = true;
        await mutationBarrier;
      });
      const activeUse = await manager.acquireUse('Xenova/whisper-small');
      await expect(manager.deleteIfIdle('Xenova/whisper-small')).resolves.toMatchObject({
        outcome: 'in-use',
        status: { state: 'ready' },
      });
      expect(mutationEntered).toBe(false);

      const deletion = manager.delete('Xenova/whisper-small');
      await new Promise((resolveWait) => setTimeout(resolveWait, 20));
      expect(mutationEntered).toBe(false);
      activeUse.release();
      await waitForCondition(() => mutationEntered);

      let newUseSettled = false;
      const newUse = manager.acquireUse('Xenova/whisper-small').then((grant) => {
        newUseSettled = true;
        return grant;
      });
      await new Promise((resolveWait) => setTimeout(resolveWait, 20));
      expect(newUseSettled).toBe(false);
      allowMutation();
      expect((await deletion).state).toBe('missing');
      const afterDelete = await newUse;
      expect(afterDelete.status.state).toBe('missing');
      afterDelete.release();
    } finally {
      await server.close();
    }
  });

  it('allows mutation of one model while another model download is waiting on the network', async () => {
    const root = await createTestDirectory('model-independent-mutations');
    roots.push(root);
    let fetchStarted = false;
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      validateRequestUrl: () => true,
      fetch: (_input, init) => {
        fetchStarted = true;
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                init?.signal?.addEventListener(
                  'abort',
                  () => controller.error(new Error('cancelled')),
                  { once: true },
                );
              },
            }),
            { status: 200 },
          ),
        );
      },
    });
    const downloading = manager.download('Xenova/whisper-small');
    await waitForCondition(() => fetchStarted);

    await expect(
      Promise.race([
        manager.deleteIfIdle('onnx-community/whisper-large-v3-turbo'),
        new Promise((_, reject) =>
          setTimeout(() => reject(new Error('unrelated model mutation was blocked')), 250),
        ),
      ]),
    ).resolves.toMatchObject({ outcome: 'deleted' });

    await manager.cancel('Xenova/whisper-small');
    await expect(downloading).resolves.toMatchObject({ state: 'missing' });
  });

  it('recovers interrupted replacement backups and removes stale replaced artifacts', async () => {
    const root = await createTestDirectory('model-publication-recovery');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      await createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      ).download('Xenova/whisper-small');
      const first = files[0];
      if (first === undefined) throw new Error('Fixture missing');
      const directory = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
      const backup = `${directory}.replaced`;
      await rename(directory, backup);

      const recovered = createManager(root, () => 'https://unused.invalid');
      await recovered.initialize();
      const expectedFirst = bodies.get(first.path);
      if (expectedFirst === undefined) throw new Error('Fixture missing');
      expect((await readFile(join(directory, first.path))).equals(expectedFirst)).toBe(true);
      await expect(readFile(join(backup, first.path))).rejects.toMatchObject({ code: 'ENOENT' });
      expect((await recovered.status('Xenova/whisper-small', true)).state).toBe('ready');

      await mkdir(backup, { recursive: true });
      await writeFile(join(backup, 'stale.txt'), 'stale');
      const cleanup = createManager(root, () => 'https://unused.invalid');
      await cleanup.initialize();
      await expect(readFile(join(backup, 'stale.txt'))).rejects.toMatchObject({ code: 'ENOENT' });
    } finally {
      await server.close();
    }
  });

  it('keeps completed files inside the staged revision until one atomic directory publish', async () => {
    const root = await createTestDirectory('model-staged-publication');
    roots.push(root);
    const secondFile = files[1];
    if (secondFile === undefined) throw new Error('Fixture missing');
    let releaseSecond: () => void = () => undefined;
    let secondRequested = false;
    const stagedFetch: typeof fetch = (input) => {
      const url = new URL(input instanceof Request ? input.url : String(input));
      const index = Number(url.pathname.slice(1));
      const file = files[index];
      if (file === undefined) return Promise.resolve(new Response(null, { status: 404 }));
      const body = bodies.get(file.path);
      if (body === undefined) return Promise.reject(new Error('Fixture missing'));
      if (index === 1) {
        secondRequested = true;
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                releaseSecond = () => {
                  controller.enqueue(body);
                  controller.close();
                };
              },
            }),
            { status: 200 },
          ),
        );
      }
      return Promise.resolve(new Response(body, { status: 200 }));
    };
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      fetch: stagedFetch,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: (_model, file) =>
        `https://trusted.example/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      validateRequestUrl: () => true,
    });
    const downloading = manager.download('Xenova/whisper-small');
    await waitForCondition(() => secondRequested);
    const stagedFirst = join(
      root,
      'models',
      '.tmp',
      'Xenova',
      'whisper-small',
      smallRevision,
      files[0]?.path ?? '',
    );
    const publishedFirst = join(
      root,
      'models',
      'Xenova',
      'whisper-small',
      smallRevision,
      files[0]?.path ?? '',
    );
    expect(await readFile(stagedFirst)).toEqual(bodies.get(files[0]?.path ?? ''));
    await expect(readFile(publishedFirst)).rejects.toMatchObject({ code: 'ENOENT' });
    releaseSecond();
    await expect(downloading).resolves.toMatchObject({ state: 'ready' });
    expect(await readFile(publishedFirst)).toEqual(bodies.get(files[0]?.path ?? ''));
    await expect(readFile(stagedFirst)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('cancels during verification without a false Ready state and reuses every verified staged file', async () => {
    const root = await createTestDirectory('model-cancel-verification');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      const controller = new AbortController();
      const states: string[] = [];
      const manager = createManager(
        root,
        (file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
      );
      manager.subscribe((event) => {
        states.push(event.state);
        if (event.state === 'verifying') controller.abort('test verification cancellation');
      });
      await expect(
        manager.download('Xenova/whisper-small', controller.signal),
      ).rejects.toMatchObject({ code: 'CANCELLED' });
      expect(states).toContain('verifying');
      expect(states).not.toContain('ready');
      await server.close();

      let fetched = false;
      const resumed = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        availableBytes: () => Promise.resolve(0),
        fetch: () => {
          fetched = true;
          return Promise.reject(new Error('verified staging must not be downloaded again'));
        },
        validateRequestUrl: () => true,
      });
      await expect(resumed.download('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
      expect(fetched).toBe(false);
    } finally {
      await server.close().catch(() => undefined);
    }
  });

  it.runIf(process.platform === 'win32')(
    'retries transient Windows activation locks without downloading verified files again',
    async () => {
      const root = await createTestDirectory('model-windows-activation-retry');
      roots.push(root);
      const server = await startRangeServer(
        files.map((file, index) => ({
          path: `/${String(index)}`,
          body: bodies.get(file.path) ?? new Uint8Array(),
        })),
      );
      try {
        const target = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
        let activationAttempts = 0;
        const manager = new ModelManager({
          modelsDirectory: join(root, 'models'),
          temporaryDirectory: join(root, 'models', '.tmp'),
          manifest,
          availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
          urlFor: (_model, file) =>
            `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
          validateRequestUrl: (url) => url.startsWith(server.origin),
          rename: async (source, destination) => {
            if (
              String(destination) === target &&
              String(source).includes(`${join('models', '.tmp')}\\`)
            ) {
              activationAttempts += 1;
              if (activationAttempts < 3) {
                throw Object.assign(new Error('fixture file lock'), { code: 'EPERM' });
              }
            }
            await rename(source, destination);
          },
        });
        await expect(manager.download('Xenova/whisper-small')).resolves.toMatchObject({
          state: 'ready',
        });
        expect(activationAttempts).toBe(3);
      } finally {
        await server.close();
      }
    },
  );

  it.runIf(process.platform === 'win32')(
    'retains a complete staged revision when a Windows activation lock outlasts retries',
    async () => {
      const root = await createTestDirectory('model-windows-activation-locked');
      roots.push(root);
      const server = await startRangeServer(
        files.map((file, index) => ({
          path: `/${String(index)}`,
          body: bodies.get(file.path) ?? new Uint8Array(),
        })),
      );
      const target = join(root, 'models', 'Xenova', 'whisper-small', smallRevision);
      try {
        const locked = new ModelManager({
          modelsDirectory: join(root, 'models'),
          temporaryDirectory: join(root, 'models', '.tmp'),
          manifest,
          availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
          urlFor: (_model, file) =>
            `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
          validateRequestUrl: (url) => url.startsWith(server.origin),
          rename: async (source, destination) => {
            if (
              String(destination) === target &&
              String(source).includes(`${join('models', '.tmp')}\\`)
            ) {
              throw Object.assign(new Error('fixture file lock'), { code: 'EBUSY' });
            }
            await rename(source, destination);
          },
        });
        await expect(locked.download('Xenova/whisper-small')).rejects.toMatchObject({
          code: 'FILE_LOCKED',
          repairable: true,
        });
        const stagedFirst = join(
          root,
          'models',
          '.tmp',
          'Xenova',
          'whisper-small',
          smallRevision,
          files[0]?.path ?? '',
        );
        expect(await readFile(stagedFirst)).toEqual(bodies.get(files[0]?.path ?? ''));
        await server.close();

        let fetched = false;
        const resumed = new ModelManager({
          modelsDirectory: join(root, 'models'),
          temporaryDirectory: join(root, 'models', '.tmp'),
          manifest,
          availableBytes: () => Promise.resolve(0),
          fetch: () => {
            fetched = true;
            return Promise.reject(new Error('verified staging must not be downloaded again'));
          },
          validateRequestUrl: () => true,
        });
        await expect(resumed.download('Xenova/whisper-small')).resolves.toMatchObject({
          state: 'ready',
        });
        expect(fetched).toBe(false);
      } finally {
        await server.close().catch(() => undefined);
      }
    },
  );

  it('does not report Ready after worker validation fails and retries without network', async () => {
    const root = await createTestDirectory('model-worker-validation');
    roots.push(root);
    const server = await startRangeServer(
      files.map((file, index) => ({
        path: `/${String(index)}`,
        body: bodies.get(file.path) ?? new Uint8Array(),
      })),
    );
    try {
      let fetches = 0;
      let validations = 0;
      const manager = new ModelManager({
        modelsDirectory: join(root, 'models'),
        temporaryDirectory: join(root, 'models', '.tmp'),
        manifest,
        availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
        urlFor: (_model, file) =>
          `${server.origin}/${String(files.findIndex((candidate) => candidate.path === file.path))}`,
        validateRequestUrl: (url) => url.startsWith(server.origin),
        fetch: async (input, init) => {
          fetches += 1;
          return fetch(input, init);
        },
      });
      manager.setAfterInstallValidation(() => {
        validations += 1;
        return validations === 1
          ? Promise.reject(
              new WhisperClientError('WORKER_CRASHED', 'fixture worker validation failed'),
            )
          : Promise.resolve();
      });
      await expect(manager.download('Xenova/whisper-small')).rejects.toMatchObject({
        code: 'WORKER_VALIDATION',
        repairable: true,
      });
      await expect(manager.status('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'error',
        downloadedBytes: totalBytes,
      });
      const downloadedFetches = fetches;
      await expect(manager.retry('Xenova/whisper-small')).resolves.toMatchObject({
        state: 'ready',
      });
      expect(fetches).toBe(downloadedFetches);
      expect(validations).toBe(2);
    } finally {
      await server.close();
    }
  });

  it('pauses an active stream without deleting resumable partial data, then retries', async () => {
    const root = await createTestDirectory('model-pause');
    roots.push(root);
    await mkdir(join(root, 'models', '.tmp'), { recursive: true });
    let calls = 0;
    const slowFetch: typeof fetch = (_input, init) => {
      calls += 1;
      const body = bodies.get(files[0]?.path ?? '');
      if (body === undefined) return Promise.reject(new Error('Fixture missing'));
      return Promise.resolve(
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(body.subarray(0, 4));
              init?.signal?.addEventListener(
                'abort',
                () => controller.error(new Error('aborted')),
                {
                  once: true,
                },
              );
            },
          }),
          { status: 200, headers: { 'content-length': String(body.byteLength) } },
        ),
      );
    };
    const manager = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      fetch: slowFetch,
      validateRequestUrl: () => true,
    });
    let streamedBytes = 0;
    manager.subscribe((event) => {
      streamedBytes = Math.max(streamedBytes, event.total.downloadedBytes);
    });
    const downloading = manager.download('Xenova/whisper-small');
    await waitForCondition(() => streamedBytes > 0);
    await expect(manager.download('onnx-community/whisper-large-v3-turbo')).rejects.toMatchObject({
      code: 'BUSY',
    });
    const paused = await manager.pause('Xenova/whisper-small');
    expect(paused.state).toBe('paused');
    expect(paused.downloadedBytes).toBeGreaterThan(0);
    expect(await downloading).toEqual(paused);
    expect(calls).toBe(1);
    await manager.cancel('Xenova/whisper-small');
    expect((await manager.status('Xenova/whisper-small')).downloadedBytes).toBe(0);
  });

  it('rejects linked staging directories before any network contact', async () => {
    const root = await createTestDirectory('model-linked-staging');
    roots.push(root);
    const models = join(root, 'models');
    const external = join(root, 'external');
    await Promise.all([mkdir(models, { recursive: true }), mkdir(external, { recursive: true })]);
    await symlink(
      external,
      join(models, '.tmp'),
      process.platform === 'win32' ? 'junction' : 'dir',
    );
    let fetched = false;
    const manager = new ModelManager({
      modelsDirectory: models,
      temporaryDirectory: join(models, '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      validateRequestUrl: () => true,
      fetch: () => {
        fetched = true;
        return Promise.reject(new Error('must not contact network'));
      },
    });
    await expect(manager.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CORRUPT',
      repairable: true,
    });
    expect(fetched).toBe(false);
  });

  it('validates every redirect before contact and applies a combined request timeout', async () => {
    const root = await createTestDirectory('model-network-guard');
    roots.push(root);
    await mkdir(join(root, 'models', '.tmp'), { recursive: true });
    const contacted: string[] = [];
    const redirecting = new ModelManager({
      modelsDirectory: join(root, 'models'),
      temporaryDirectory: join(root, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: () => 'https://trusted.example/artifact',
      validateRequestUrl: (url) => url.startsWith('https://trusted.example/'),
      fetch: (input) => {
        contacted.push(
          input instanceof URL ? input.href : typeof input === 'string' ? input : input.url,
        );
        return Promise.resolve(
          new Response(null, {
            status: 302,
            headers: { location: 'https://untrusted.invalid/model' },
          }),
        );
      },
    });
    await expect(redirecting.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'PROTOCOL',
    });
    expect(contacted).toEqual(['https://trusted.example/artifact']);

    const timeoutRoot = await createTestDirectory('model-timeout');
    roots.push(timeoutRoot);
    await mkdir(join(timeoutRoot, 'models', '.tmp'), { recursive: true });
    const timingOut = new ModelManager({
      modelsDirectory: join(timeoutRoot, 'models'),
      temporaryDirectory: join(timeoutRoot, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: () => 'https://trusted.example/artifact',
      validateRequestUrl: () => true,
      requestTimeoutMs: 10,
      fetch: (_input, init) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener(
            'abort',
            () => reject(new DOMException('timed out', 'AbortError')),
            { once: true },
          );
        }),
    });
    await expect(timingOut.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'TIMEOUT',
      repairable: true,
    });

    const inactiveRoot = await createTestDirectory('model-inactive-body');
    roots.push(inactiveRoot);
    let inactiveCancelled = false;
    const inactive = new ModelManager({
      modelsDirectory: join(inactiveRoot, 'models'),
      temporaryDirectory: join(inactiveRoot, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: () => 'https://trusted.example/artifact',
      validateRequestUrl: () => true,
      requestTimeoutMs: 10,
      fetch: () =>
        Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              cancel() {
                inactiveCancelled = true;
              },
            }),
            { status: 200 },
          ),
        ),
    });
    await expect(inactive.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'TIMEOUT',
      repairable: true,
    });
    expect(inactiveCancelled).toBe(true);

    const shutdownRoot = await createTestDirectory('model-shutdown-body');
    roots.push(shutdownRoot);
    let shutdownBodyStarted = false;
    let shutdownCancelled = false;
    const shuttingDown = new ModelManager({
      modelsDirectory: join(shutdownRoot, 'models'),
      temporaryDirectory: join(shutdownRoot, 'models', '.tmp'),
      manifest,
      availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
      urlFor: () => 'https://trusted.example/artifact',
      validateRequestUrl: () => true,
      requestTimeoutMs: 60_000,
      fetch: () => {
        shutdownBodyStarted = true;
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              cancel() {
                shutdownCancelled = true;
              },
            }),
            { status: 200 },
          ),
        );
      },
    });
    const interrupted = shuttingDown.download('Xenova/whisper-small');
    await waitForCondition(() => shutdownBodyStarted);
    await shuttingDown.shutdown();
    await expect(interrupted).resolves.toMatchObject({ state: 'missing' });
    expect(shutdownCancelled).toBe(true);
    await expect(shuttingDown.download('Xenova/whisper-small')).rejects.toMatchObject({
      code: 'CANCELLED',
    });
  });
});

class DelayedUseCoordinator extends ModelAccessCoordinator {
  readonly events: string[] = [];
  readonly useRequested: Promise<void>;
  useAcquireCalls = 0;
  internalUseCancelled = false;
  #markUseRequested: () => void = () => undefined;
  #allowUse: () => void = () => undefined;
  readonly #useGate: Promise<void>;

  constructor() {
    super();
    this.useRequested = new Promise((resolveRequested) => {
      this.#markUseRequested = resolveRequested;
    });
    this.#useGate = new Promise((resolveUse) => {
      this.#allowUse = resolveUse;
    });
  }

  override async acquireUse(
    modelId: 'Xenova/whisper-small' | 'onnx-community/whisper-large-v3-turbo',
    signal?: AbortSignal,
  ): Promise<ModelAccessLease> {
    this.useAcquireCalls += 1;
    this.#markUseRequested();
    await this.#useGate;
    try {
      const lease = await super.acquireUse(modelId, signal);
      this.events.push('use-granted');
      return lease;
    } catch (error: unknown) {
      if (signal?.aborted === true) this.internalUseCancelled = true;
      throw error;
    }
  }

  override async acquireMutation(
    modelId: 'Xenova/whisper-small' | 'onnx-community/whisper-large-v3-turbo',
    signal?: AbortSignal,
  ): Promise<ModelAccessLease> {
    const lease = await super.acquireMutation(modelId, signal);
    this.events.push('mutation-granted');
    let released = false;
    return {
      release: () => {
        if (released) return;
        released = true;
        lease.release();
        this.events.push('mutation-released');
      },
    };
  }

  allowUseAcquisition(): void {
    this.#allowUse();
  }

  resetEvents(): void {
    this.events.length = 0;
  }
}

async function waitForCondition(condition: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!condition()) {
    if (Date.now() >= deadline) throw new Error('Timed out waiting for condition');
    await new Promise((resolveWait) => setTimeout(resolveWait, 5));
  }
}

function createManager(
  root: string,
  urlFor: (file: ModelManifestFile) => string,
  fetchModel?: typeof fetch,
): ModelManager {
  return new ModelManager({
    modelsDirectory: join(root, 'models'),
    temporaryDirectory: join(root, 'models', '.tmp'),
    manifest,
    ...(fetchModel === undefined ? {} : { fetch: fetchModel }),
    availableBytes: () => Promise.resolve(Number.MAX_SAFE_INTEGER),
    urlFor: (_model, file) => urlFor(file),
    validateRequestUrl: () => true,
  });
}
