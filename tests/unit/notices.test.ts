import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { NoticesService } from '../../app/src/main/info/notices-service';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

describe('third-party notices', () => {
  it('ships bounded generated dependency, model, and MIT attribution', async () => {
    const path = resolve('app/assets/THIRD_PARTY_NOTICES.txt');
    const text = await new NoticesService(path).read();
    expect(text).toContain('Mintplex Labs Inc.');
    expect(text).toContain('Xenova/whisper-small');
    expect(text).toContain('onnx-community/whisper-large-v3-turbo');
    expect(text).toContain('@huggingface/transformers');
    expect(text).toContain('react@19.2.7 — SPDX: MIT');
    expect(text).toContain('zod@4.4.3 — SPDX: MIT');
    expect(text).toContain('crossbeam-channel@');
    expect(text).not.toContain('aws4@');
    expect(text).not.toContain('proptest@');
    expect(text).not.toMatch(/unknown|unlicensed|see exact|license declared|placeholder/iu);
    const dependencyLines = text
      .split(/\r?\n/u)
      .filter((line) => /@[^ ]+ — SPDX: |^[a-z][^ ]+@[^ ]+ — SPDX: /u.test(line));
    expect(dependencyLines.length).toBeGreaterThan(50);
    expect(dependencyLines.every((line) => !/SPDX:\s*$/u.test(line))).toBe(true);
    expect(text).toContain('helper/Cargo.lock SHA-256:');
    expect(text).toContain('pnpm-lock.yaml SHA-256:');
    expect(text).toContain('MIT License');
    expect(text).toContain('onnxruntime-node@1.21.0/onnxruntime/v1.21.0/ThirdPartyNotices.txt');
    expect(text).toContain('guid-typescript@1.0.9/guid-typescript-1.0.9-LICENSE.txt');
    expect(text).toContain('Declared license: Apache-2.0');
    expect(text).toContain('Declared license: MIT');
    expect(text).toContain('Apache License\n                           Version 2.0, January 2004');
    expect(text).not.toContain('No separate LICENSE/NOTICE file was present');
    expect(text).not.toContain('absence requires legal review');
    expect(await readFile(path, 'utf8')).toBe(text);
  });

  it('deduplicates and caches concurrent reads of the immutable notices resource', async () => {
    const directory = await createTestDirectory('notices-cache');
    try {
      const path = join(directory, 'notices.txt');
      await writeFile(path, 'first notice');
      const service = new NoticesService(path);
      await expect(Promise.all([service.read(), service.read(), service.read()])).resolves.toEqual([
        'first notice',
        'first notice',
        'first notice',
      ]);
      await writeFile(path, 'modified after immutable read');
      await expect(service.read()).resolves.toBe('first notice');
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('rejects a notices resource larger than the actual read bound', async () => {
    const directory = await createTestDirectory('notices-bound');
    try {
      const path = join(directory, 'notices.txt');
      await writeFile(path, Buffer.alloc(2_000_001, 97));
      await expect(new NoticesService(path).read()).rejects.toThrow('Notices resource is invalid');
    } finally {
      await removeTestDirectory(directory);
    }
  });
});
