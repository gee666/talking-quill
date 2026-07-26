import { createRequire, syncBuiltinESMExports } from 'node:module';

const GUARD_SYMBOL = Symbol.for('talking-quill.whisper-network-guard');
const BLOCKED_MESSAGE = 'Network access is disabled in the Whisper worker.';
const DNS_CALLBACK_METHODS = [
  'lookup',
  'lookupService',
  'resolve',
  'resolve4',
  'resolve6',
  'resolveAny',
  'resolveCaa',
  'resolveCname',
  'resolveMx',
  'resolveNaptr',
  'resolveNs',
  'resolvePtr',
  'resolveSoa',
  'resolveSrv',
  'resolveTlsa',
  'resolveTxt',
  'reverse',
] as const;
const DNS_RESOLVER_METHODS = [
  'resolve',
  'resolve4',
  'resolve6',
  'resolveAny',
  'resolveCaa',
  'resolveCname',
  'resolveMx',
  'resolveNaptr',
  'resolveNs',
  'resolvePtr',
  'resolveSoa',
  'resolveSrv',
  'resolveTlsa',
  'resolveTxt',
  'reverse',
] as const;

export interface WorkerNetworkGuardState {
  readonly installed: true;
  readonly blockedAttempts: number;
  readonly probeCompleted: boolean;
}

type UnknownRecord = Record<PropertyKey, unknown>;
type UnknownFunction = (...arguments_: unknown[]) => unknown;

let markProbeCompleted: (() => void) | null = null;

export function installWorkerNetworkGuard(): WorkerNetworkGuardState {
  const globals = globalThis as typeof globalThis & {
    [GUARD_SYMBOL]?: WorkerNetworkGuardState;
  };
  const existing = globals[GUARD_SYMBOL];
  if (existing !== undefined) return existing;

  let blockedAttempts = 0;
  let probeCompleted = false;
  const blocked = function blockedNetworkOperation(): never {
    blockedAttempts += 1;
    throw new Error(BLOCKED_MESSAGE);
  };
  const blockedAsync = function blockedAsyncNetworkOperation(): Promise<never> {
    blockedAttempts += 1;
    return Promise.reject(new Error(BLOCKED_MESSAGE));
  };
  const state = Object.freeze({
    installed: true as const,
    get blockedAttempts() {
      return blockedAttempts;
    },
    get probeCompleted() {
      return probeCompleted;
    },
  });
  markProbeCompleted = () => {
    probeCompleted = true;
  };

  defineBlocked(globalThis, 'fetch', blockedAsync);
  for (const globalName of ['WebSocket', 'EventSource'] as const) {
    if (globalName in globalThis) defineBlocked(globalThis, globalName, blocked);
  }

  const require = createRequire(__filename);
  const http = requireRecord(require, 'node:http');
  const https = requireRecord(require, 'node:https');
  const http2 = requireRecord(require, 'node:http2');
  const net = requireRecord(require, 'node:net');
  const tls = requireRecord(require, 'node:tls');
  const dgram = requireRecord(require, 'node:dgram');
  const dns = requireRecord(require, 'node:dns');
  const dnsPromises = requireRecord(require, 'node:dns/promises');
  const inspector = requireRecord(require, 'node:inspector');
  const inspectorPromises = requireRecord(require, 'node:inspector/promises');
  const childProcess = requireRecord(require, 'node:child_process');
  const workerThreads = requireRecord(require, 'node:worker_threads');
  const cluster = requireRecord(require, 'node:cluster');
  const electron = tryRequireRecord(require, 'electron');

  patchMethods(http, ['request', 'get', 'createServer', 'ClientRequest', 'WebSocket'], blocked);
  patchPrototypeMethods(http, 'Agent', ['createConnection'], blocked);
  patchMethods(https, ['request', 'get', 'createServer'], blocked);
  patchPrototypeMethods(https, 'Agent', ['createConnection'], blocked);
  patchMethods(http2, ['connect', 'createServer', 'createSecureServer'], blocked);
  patchMethods(
    net,
    ['connect', 'createConnection', 'createServer', '_createServerHandle'],
    blocked,
  );
  patchPrototypeMethods(net, 'Socket', ['connect'], blocked);
  patchPrototypeMethods(net, 'Server', ['listen'], blocked);
  patchMethods(tls, ['connect', 'createServer'], blocked);
  patchPrototypeMethods(tls, 'TLSSocket', ['connect', '_start'], blocked);
  patchPrototypeMethods(tls, 'Server', ['listen'], blocked);
  patchPrototypeMethods(dgram, 'Socket', ['bind', 'connect', 'send'], blocked);
  patchMethods(dgram, ['createSocket', 'Socket', '_createSocketHandle'], blocked);
  patchMethods(dns, DNS_CALLBACK_METHODS, blocked);
  patchPrototypeMethods(dns, 'Resolver', DNS_RESOLVER_METHODS, blocked);
  patchMethods(dns, ['Resolver'], blocked);
  patchMethods(dnsPromises, DNS_CALLBACK_METHODS, blockedAsync);
  patchPrototypeMethods(dnsPromises, 'Resolver', DNS_RESOLVER_METHODS, blockedAsync);
  patchMethods(dnsPromises, ['Resolver'], blocked);
  patchMethods(inspector, ['open'], blocked);
  patchMethods(inspectorPromises, ['open'], blocked);
  patchMethods(
    childProcess,
    ['exec', 'execFile', 'execFileSync', 'execSync', 'fork', 'spawn', 'spawnSync'],
    blocked,
  );
  patchMethods(workerThreads, ['Worker'], blocked);
  patchMethods(cluster, ['fork', 'setupMaster', 'setupPrimary'], blocked);
  if (electron !== null) {
    const electronNet = Reflect.get(electron, 'net');
    if (typeof electronNet === 'object' && electronNet !== null) {
      patchMethods(electronNet as UnknownRecord, ['fetch', 'request', 'resolveHost'], blocked);
    }
  }

  installGuardedGetBuiltinModule(
    new Map([
      ['http', http],
      ['https', https],
      ['http2', http2],
      ['net', net],
      ['tls', tls],
      ['dgram', dgram],
      ['dns', dns],
      ['dns/promises', dnsPromises],
      ['inspector', inspector],
      ['inspector/promises', inspectorPromises],
      ['child_process', childProcess],
      ['worker_threads', workerThreads],
      ['cluster', cluster],
    ]),
  );
  syncBuiltinESMExports();

  Object.defineProperty(globals, GUARD_SYMBOL, {
    configurable: false,
    enumerable: false,
    writable: false,
    value: state,
  });
  return state;
}

export async function verifyWorkerNetworkGuardForTest(): Promise<void> {
  const state = installWorkerNetworkGuard();
  const require = createRequire(__filename);
  const http = requireRecord(require, 'http');
  const https = requireRecord(require, 'node:https');
  const http2 = requireRecord(require, 'http2');
  const net = requireRecord(require, 'node:net');
  const tls = requireRecord(require, 'tls');
  const dgram = requireRecord(require, 'node:dgram');
  const dns = requireRecord(require, 'dns');
  const dnsPromises = requireRecord(require, 'node:dns/promises');
  const inspector = requireRecord(require, 'inspector');
  const inspectorPromises = requireRecord(require, 'node:inspector/promises');
  const childProcess = requireRecord(require, 'child_process');
  const workerThreads = requireRecord(require, 'node:worker_threads');
  const cluster = requireRecord(require, 'cluster');

  await expectAsynchronousBlock(() => globalThis.fetch('http://127.0.0.1'));
  for (const globalName of ['WebSocket', 'EventSource'] as const) {
    const constructor = Reflect.get(globalThis, globalName);
    if (typeof constructor === 'function') {
      expectSynchronousBlock(() => Reflect.construct(constructor, ['ws://127.0.0.1']));
    }
  }

  probeMethods(http, ['request', 'get', 'createServer']);
  probeConstructor(http, 'ClientRequest');
  probeConstructor(http, 'WebSocket');
  probePrototypeMethods(http, 'Agent', ['createConnection']);
  probeMethods(https, ['request', 'get', 'createServer']);
  probePrototypeMethods(https, 'Agent', ['createConnection']);
  probeMethods(http2, ['connect', 'createServer', 'createSecureServer']);
  probeMethods(net, ['connect', 'createConnection', 'createServer', '_createServerHandle']);
  probePrototypeMethods(net, 'Socket', ['connect']);
  probePrototypeMethods(net, 'Server', ['listen']);
  probeMethods(tls, ['connect', 'createServer']);
  probePrototypeMethods(tls, 'TLSSocket', ['connect', '_start']);
  probePrototypeMethods(tls, 'Server', ['listen']);
  probeMethods(dgram, ['createSocket', '_createSocketHandle']);
  probeConstructor(dgram, 'Socket');
  probeMethods(dns, DNS_CALLBACK_METHODS);
  probeConstructor(dns, 'Resolver');
  await probeAsynchronousMethods(dnsPromises, DNS_CALLBACK_METHODS);
  probeConstructor(dnsPromises, 'Resolver');
  probeMethods(inspector, ['open']);
  probeMethods(inspectorPromises, ['open']);
  probeMethods(childProcess, [
    'exec',
    'execFile',
    'execFileSync',
    'execSync',
    'fork',
    'spawn',
    'spawnSync',
  ]);
  probeConstructor(workerThreads, 'Worker');
  probeMethods(cluster, ['fork', 'setupMaster', 'setupPrimary']);

  const processRecord = process as unknown as UnknownRecord;
  const getBuiltinModule = readFunction(processRecord, 'getBuiltinModule');
  for (const moduleName of ['http', 'node:http']) {
    const builtinHttp = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeMethods(builtinHttp, ['request']);
    probeConstructor(builtinHttp, 'WebSocket');
  }
  for (const moduleName of ['net', 'node:net']) {
    const builtinNet = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeMethods(builtinNet, ['connect', '_createServerHandle']);
  }
  for (const moduleName of ['dgram', 'node:dgram']) {
    const builtinDgram = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeMethods(builtinDgram, ['createSocket', '_createSocketHandle']);
    probeConstructor(builtinDgram, 'Socket');
  }
  for (const moduleName of ['dns', 'node:dns', 'dns/promises', 'node:dns/promises']) {
    const builtinDns = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeConstructor(builtinDns, 'Resolver');
  }
  for (const moduleName of [
    'inspector',
    'node:inspector',
    'inspector/promises',
    'node:inspector/promises',
  ]) {
    const builtinInspector = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeMethods(builtinInspector, ['open']);
  }
  for (const moduleName of ['child_process', 'node:child_process']) {
    const builtinChildProcess = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeMethods(builtinChildProcess, ['exec', 'spawn']);
  }
  for (const moduleName of ['worker_threads', 'node:worker_threads']) {
    const builtinWorkerThreads = asRecord(Reflect.apply(getBuiltinModule, process, [moduleName]));
    probeConstructor(builtinWorkerThreads, 'Worker');
  }

  const importedNodeHttp = asRecord(await import('node:http'));
  const importedAliasHttp = asRecord(await import('http'));
  const importedNodeNet = asRecord(await import('node:net'));
  const importedAliasNet = asRecord(await import('net'));
  const importedDns = asRecord(await import('node:dns'));
  const importedAliasDns = asRecord(await import('dns'));
  const importedDgram = asRecord(await import('node:dgram'));
  const importedAliasDgram = asRecord(await import('dgram'));
  const importedInspector = asRecord(await import('node:inspector'));
  const importedAliasInspector = asRecord(await import('inspector'));
  const importedInspectorPromises = asRecord(await import('node:inspector/promises'));
  const importedChildProcess = asRecord(await import('node:child_process'));
  const importedWorkerThreads = asRecord(await import('node:worker_threads'));
  for (const importedHttp of [importedNodeHttp, importedAliasHttp]) {
    probeMethods(importedHttp, ['request', 'get']);
    probeConstructor(importedHttp, 'WebSocket');
  }
  for (const importedNet of [importedNodeNet, importedAliasNet]) {
    probeMethods(importedNet, ['connect', '_createServerHandle']);
  }
  for (const importedResolver of [importedDns, importedAliasDns]) {
    probeMethods(importedResolver, ['resolve']);
    probeConstructor(importedResolver, 'Resolver');
  }
  for (const importedSocket of [importedDgram, importedAliasDgram]) {
    probeMethods(importedSocket, ['createSocket', '_createSocketHandle']);
    probeConstructor(importedSocket, 'Socket');
  }
  for (const importedInspectorModule of [
    importedInspector,
    importedAliasInspector,
    importedInspectorPromises,
  ]) {
    probeMethods(importedInspectorModule, ['open']);
  }
  probeMethods(importedChildProcess, ['exec', 'spawn']);
  probeConstructor(importedWorkerThreads, 'Worker');

  if (state.blockedAttempts === 0 || markProbeCompleted === null) {
    throw new Error('Whisper network guard verification failed.');
  }
  markProbeCompleted();
}

function installGuardedGetBuiltinModule(modules: ReadonlyMap<string, UnknownRecord>): void {
  const processRecord = process as unknown as UnknownRecord;
  const original = readFunction(processRecord, 'getBuiltinModule');
  const guardedGetBuiltinModule = function guardedGetBuiltinModule(name: unknown): unknown {
    if (typeof name !== 'string') return Reflect.apply(original, process, [name]);
    const canonical = name.startsWith('node:') ? name.slice(5) : name;
    return modules.get(canonical) ?? Reflect.apply(original, process, [name]);
  };
  defineBlocked(processRecord, 'getBuiltinModule', guardedGetBuiltinModule);
}

function requireRecord(require: NodeJS.Require, name: string): UnknownRecord {
  return asRecord(require(name));
}

function tryRequireRecord(require: NodeJS.Require, name: string): UnknownRecord | null {
  try {
    return asRecord(require(name));
  } catch {
    return null;
  }
}

function asRecord(value: unknown): UnknownRecord {
  if ((typeof value !== 'object' || value === null) && typeof value !== 'function') {
    throw new Error('Whisper network guard initialization failed.');
  }
  return value as UnknownRecord;
}

function readFunction(target: UnknownRecord, name: string): UnknownFunction {
  const value = Reflect.get(target, name);
  if (typeof value !== 'function') throw new Error('Whisper network guard initialization failed.');
  return value as UnknownFunction;
}

function defineBlocked(target: object, name: PropertyKey, replacement: unknown): void {
  Object.defineProperty(target, name, {
    configurable: false,
    enumerable: true,
    writable: false,
    value: replacement,
  });
}

function patchMethods(target: UnknownRecord, names: readonly string[], replacement: unknown): void {
  for (const name of names) {
    if (name in target) defineBlocked(target, name, replacement);
  }
}

function patchPrototypeMethods(
  target: UnknownRecord,
  constructorName: string,
  names: readonly string[],
  replacement: unknown,
): void {
  const constructor: unknown = Reflect.get(target, constructorName);
  if (typeof constructor !== 'function') return;
  const prototype: unknown = Reflect.get(constructor, 'prototype');
  if (typeof prototype !== 'object' || prototype === null) return;
  patchMethods(prototype as UnknownRecord, names, replacement);
}

function probeMethods(target: UnknownRecord, names: readonly string[]): void {
  for (const name of names) {
    if (!(name in target)) continue;
    const operation = readFunction(target, name);
    expectSynchronousBlock(() => Reflect.apply(operation, target, []));
  }
}

function probeConstructor(target: UnknownRecord, name: string): void {
  const constructor = Reflect.get(target, name);
  if (typeof constructor !== 'function') return;
  expectSynchronousBlock(() => Reflect.construct(constructor, []));
}

function probePrototypeMethods(
  target: UnknownRecord,
  constructorName: string,
  names: readonly string[],
): void {
  const constructor: unknown = Reflect.get(target, constructorName);
  if (typeof constructor !== 'function') return;
  const prototype: unknown = Reflect.get(constructor, 'prototype');
  if (typeof prototype !== 'object' || prototype === null) return;
  for (const name of names) {
    if (!(name in prototype)) continue;
    const operation = readFunction(prototype as UnknownRecord, name);
    expectSynchronousBlock(() => Reflect.apply(operation, Object.create(prototype), []));
  }
}

async function probeAsynchronousMethods(
  target: UnknownRecord,
  names: readonly string[],
): Promise<void> {
  for (const name of names) {
    if (!(name in target)) continue;
    const operation = readFunction(target, name);
    await expectAsynchronousBlock(() => Reflect.apply(operation, target, []));
  }
}

function expectSynchronousBlock(operation: () => unknown): void {
  try {
    operation();
  } catch (error: unknown) {
    if (isBlockedError(error)) return;
    throw new Error('Whisper network guard verification failed.');
  }
  throw new Error('Whisper network guard verification failed.');
}

async function expectAsynchronousBlock(operation: () => unknown): Promise<void> {
  let result: unknown;
  try {
    result = operation();
  } catch (error: unknown) {
    if (isBlockedError(error)) return;
    throw new Error('Whisper network guard verification failed.');
  }
  try {
    await Promise.resolve(result);
  } catch (error: unknown) {
    if (isBlockedError(error)) return;
    throw new Error('Whisper network guard verification failed.');
  }
  throw new Error('Whisper network guard verification failed.');
}

function isBlockedError(error: unknown): boolean {
  return error instanceof Error && error.message === BLOCKED_MESSAGE;
}
