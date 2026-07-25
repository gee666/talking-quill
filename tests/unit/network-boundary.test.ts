import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  APPROVED_NETWORK_BOUNDARIES,
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
    expect(inventory).toHaveLength(10);
    expect(Object.keys(APPROVED_NETWORK_BOUNDARIES)).toEqual(
      expect.arrayContaining([
        'app/src/main/providers/json-transport.ts',
        'app/src/main/transcription/model-manager.ts',
        'app/src/workers/whisper/network-guard.ts',
        'app/src/main/helper/helper-client.ts',
        'app/src/main/providers/pi-discovery.ts',
        'app/src/main/providers/pi.ts',
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
    expect(detectNetworkTokens(`const fetch = () => 'local value'; fetch();`)).toEqual([]);
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
