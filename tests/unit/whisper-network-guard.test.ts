import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('Whisper worker network boundary', () => {
  it('keeps the bootstrap minimal and loads the third-party payload only after guard installation', async () => {
    const bootstrap = await readFile('app/src/workers/whisper/bootstrap.ts', 'utf8');
    const guard = bootstrap.indexOf('installWorkerNetworkGuard();');
    const payload = bootstrap.indexOf("createRequire(__filename)('./whisper-payload.cjs')");
    expect(guard).toBeGreaterThanOrEqual(0);
    expect(payload).toBeGreaterThan(guard);
    for (const forbidden of ['@huggingface/transformers', 'onnxruntime-node', 'zod']) {
      expect(bootstrap).not.toContain(forbidden);
    }

    const productionPayload = await readFile('app/src/workers/whisper/index.ts', 'utf8');
    expect(productionPayload).toContain("await import('@huggingface/transformers')");
    expect(productionPayload).not.toContain('installWorkerNetworkGuard');
  });

  it('covers fresh built-in access and constructor/prototype network paths', async () => {
    const source = await readFile('app/src/workers/whisper/network-guard.ts', 'utf8');
    for (const required of [
      'syncBuiltinESMExports',
      'getBuiltinModule',
      'ClientRequest',
      'Socket',
      'TLSSocket',
      'Resolver',
      'node:dns/promises',
      'node:inspector/promises',
      "'_createServerHandle'",
      "'_createSocketHandle'",
      "patchMethods(dgram, ['createSocket', 'Socket', '_createSocketHandle'], blocked)",
      "patchMethods(dns, ['Resolver'], blocked)",
      "patchMethods(dnsPromises, ['Resolver'], blocked)",
      "patchMethods(inspector, ['open'], blocked)",
      "probeConstructor(builtinHttp, 'WebSocket')",
      "probeMethods(builtinNet, ['connect', '_createServerHandle'])",
      "await import('node:http')",
      "await import('http')",
      "await import('node:net')",
      "await import('net')",
      "await import('node:dgram')",
      "await import('dgram')",
      "await import('node:inspector')",
      "await import('inspector')",
      "await import('node:child_process')",
      "await import('node:worker_threads')",
      "tryRequireRecord(require, 'electron')",
      "'execFileSync'",
      "'worker_threads'",
      "patchMethods(workerThreads, ['Worker'], blocked)",
      'EventSource',
    ]) {
      expect(source).toContain(required);
    }
    expect(source).toMatch(/patchMethods\(\s*http,[\s\S]*?'WebSocket'/);
    expect(source).toMatch(/patchMethods\(\s*net,[\s\S]*?'_createServerHandle'/);
  });
});
