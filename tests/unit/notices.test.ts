import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { NoticesService } from '../../app/src/main/info/notices-service';

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
});
