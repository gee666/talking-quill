import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  APPROVED_NETWORK_BOUNDARIES,
  childProcessImportMembers,
  detectNetworkTokens,
  verifyNetworkBoundary,
} from '../../scripts/network-boundary-policy.mjs';
import {
  createEgressProofObserver,
  EGRESS_CATEGORIES,
} from '../../app/src/main/security/egress-audit';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

describe('closed networking boundary and privacy-safe egress proof', () => {
  let temporary = '';

  beforeEach(async () => {
    temporary = await createTestDirectory('egress-proof');
  });

  afterEach(async () => {
    await removeTestDirectory(temporary);
  });

  it('keeps every production networking primitive in the reviewed closed inventory', async () => {
    const inventory = await verifyNetworkBoundary();
    expect(inventory).toHaveLength(16);
    expect(Object.keys(APPROVED_NETWORK_BOUNDARIES)).toEqual(
      expect.arrayContaining([
        'app/src/main/providers/json-transport.ts',
        'app/src/main/transcription/model-download-transport.ts',
        'app/src/workers/whisper/network-guard.ts',
        'app/src/main/helper/helper-client.ts',
        'app/src/main/providers/pi-process-runtime.ts',
        'app/src/main/providers/pi-rpc-operation.ts',
        'app/src/main/providers/pi-rpc-transport.ts',
        'app/src/main/app/application.ts',
        'app/src/main/app/windows-uninstall-target.ts',
        'app/src/main/info/electron-update-backend.ts',
        'app/src/main/info/macos-owner-update-coordinator.ts',
      ]),
    );
    for (const [path, approval] of Object.entries(APPROVED_NETWORK_BOUNDARIES)) {
      expect(path).not.toMatch(/[*!]/u);
      expect(approval.tokens).not.toContain('*');
    }
    expect(
      detectNetworkTokens(`
        const docs = 'https://example.com/net/http/tls';
        const label = 'fetch(request) and websocket docs';
      `),
    ).toEqual([]);
    expect(
      detectNetworkTokens(`
        import { request } from 'node:https';
        import { lookup } from 'node:dns/promises';
        import { fetch as undiciFetch } from 'undici';
        const { request: electronRequest } = net;
        const socket = new WebSocket('wss://example.com');
        fetch('https://example.com');
      `),
    ).toEqual([
      'electron-net-request',
      'fetch-call',
      'node:dns/promises',
      'node:https',
      'undici',
      'websocket',
    ]);
    expect(
      detectNetworkTokens(`
        import { net as electronNetwork } from 'electron';
        import * as socketNamespace from 'node:net';
        const first = globalThis['fetch'];
        let second;
        second = first;
        const wrapper = (capability: typeof fetch) => capability('https://example.com');
        wrapper(second);
        electronNetwork['request']('https://example.com');
        const connect = socketNamespace.connect;
        connect(443);
        const beacon = navigator['sendBeacon'];
        beacon('/audit');
        const Ws = window.WebSocket;
        new Ws('wss://example.com');
        const dynamic = await import('node:tls');
        dynamic.connect(443);
      `),
    ).toEqual([
      'direct-socket-call',
      'electron-net-request',
      'fetch-call',
      'node:net',
      'node:tls',
      'send-beacon',
      'websocket',
    ]);
    expect(
      detectNetworkTokens(`
        const bound = fetch.bind(globalThis);
        bound('/bound');
        fetch.call(globalThis, '/call');
        fetch.apply(globalThis, ['/apply']);
        const holder = { request: fetch };
        holder.request('/property');
        let assigned = {};
        assigned['request'] = fetch;
        assigned.request('/assigned');
        function wrapper() { return fetch; }
        wrapper()('/wrapper');
        const computedFetch = 'fetch';
        globalThis[computedFetch]('/computed');
        let mutableFetch = 'safe';
        mutableFetch = 'fetch';
        globalThis[mutableFetch]('/mutable');
      `),
    ).toEqual(['fetch-call']);
    expect(
      detectNetworkTokens(`
        const moduleName = 'node:' + 'https';
        const dynamic = await import(moduleName);
        dynamic.request('https://example.com');
        const load = require;
        const udp = load('node:dgram');
        udp.createSocket('udp4');
        process.getBuiltinModule('node:http').request('https://example.com');
        const reflected = Reflect.get(globalThis, 'fetch');
        reflected('https://example.com');
        const getFetch = () => globalThis.fetch;
        getFetch()('https://example.com');
      `),
    ).toEqual(['direct-socket-call', 'fetch-call', 'node:dgram', 'node:http', 'node:https']);
    expect(
      detectNetworkTokens(`
        function local(require: unknown, fetch: () => void) { fetch(); }
        require('node:https').request('https://example.com');
        fetch('https://example.com');
      `),
    ).toEqual(['direct-socket-call', 'fetch-call', 'node:https']);
    expect(
      detectNetworkTokens(`
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        const udp = require('node:dgram');
        udp.createSocket('udp4');
      `),
    ).toEqual(['node:dgram']);
    expect(
      detectNetworkTokens(`
        import * as moduleApi from 'node:module';
        const require = moduleApi.createRequire(import.meta.url);
        require('node:https').request('https://example.com');
      `),
    ).toEqual(['direct-socket-call', 'node:https']);
    expect(
      detectNetworkTokens(`
        const moduleApi = await import('node:module');
        const require = moduleApi.createRequire(import.meta.url);
        require('node:dgram').createSocket('udp4');
      `),
    ).toEqual(['node:dgram']);
    expect(
      detectNetworkTokens(`
        for (let fetch = () => {}; false;) { fetch(); }
        fetch('/after-loop');
        try {} catch (fetch) { fetch(); }
        fetch('/after-catch');
        switch (1) { case 1: let fetch = () => {}; fetch(); }
        fetch('/after-switch');
      `),
    ).toEqual(['fetch-call']);
    expect(detectNetworkTokens(`const fetch = () => 'local value'; fetch();`)).toEqual([]);
  }, 30_000);

  it('keeps lifecycle child-process imports and purposes in an exact closed inventory', async () => {
    const expected = {
      'app/src/main/app/application.ts': ['execFileSync'],
      'app/src/main/info/electron-update-backend.ts': ['spawn'],
      'app/src/main/info/macos-owner-update-coordinator.ts': ['execFile', 'spawn'],
    } as const;
    for (const [path, members] of Object.entries(expected)) {
      const source = await readFile(resolve(path), 'utf8');
      expect(childProcessImportMembers(source, path)).toEqual(members);
      const approval = APPROVED_NETWORK_BOUNDARIES[path];
      if (approval === undefined) throw new Error(`Missing reviewed boundary: ${path}`);
      expect(approval.reason).toMatch(/lifecycle|maintenance|updater/iu);
    }
    const application = await readFile('app/src/main/app/application.ts', 'utf8');
    expect(application.match(/execFileSync\(/gu)).toHaveLength(2);
    expect(application).toContain("['--macos-owner-resume-cleanup']");
    expect(application).toContain("['--macos-owner-validate-install']");
    const coordinator = await readFile(
      'app/src/main/info/macos-owner-update-coordinator.ts',
      'utf8',
    );
    expect(coordinator).toContain("execFileAsync('/usr/bin/ditto'");
    expect(coordinator).toContain("['--macos-owner-finalize', ...arguments_]");
    const updater = await readFile('app/src/main/info/electron-update-backend.ts', 'utf8');
    expect(updater.match(/\bspawn\(/gu)).toHaveLength(1);
    expect(updater).toContain('spawn(executable, arguments_');
  });

  it('scans TypeScript module extensions instead of silently omitting them', async () => {
    await writeFile(resolve(temporary, 'unapproved-boundary.mts'), "import 'node:dgram';\n");
    await expect(verifyNetworkBoundary(temporary)).rejects.toThrow(
      'unapproved-boundary.mts: node:dgram',
    );
  });

  it('records category only and blocks deterministic proof traffic before socket I/O', async () => {
    const path = resolve(temporary, 'egress.jsonl');
    const observe = createEgressProofObserver(path, true);
    for (const category of EGRESS_CATEGORIES) {
      expect(() => observe(category)).toThrow(`blocked ${category} before socket I/O`);
    }
    const events = (await readFile(path, 'utf8'))
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(events).toEqual(EGRESS_CATEGORIES.map((category) => ({ schemaVersion: 1, category })));
    const source = JSON.stringify(events);
    for (const forbidden of ['url', 'host', 'header', 'body', 'transcript', 'credential']) {
      expect(source.toLowerCase()).not.toContain(forbidden);
    }
  });
});
