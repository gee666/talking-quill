import type { ChildProcessWithoutNullStreams, SpawnOptionsWithoutStdio } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { mkdir, realpath, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import { PiProvider } from '../../app/src/main/providers/pi';
import type { PiCliIdentity } from '../../app/src/main/providers/pi-executable';
import {
  PI_RPC_PROTOCOL_VERSION,
  PI_RPC_REQUIRED_SAFETY_FLAGS,
  PiRpcTimingStage,
} from '../../app/src/main/providers/pi-rpc-operation';
import type { SpawnPi } from '../../app/src/main/providers/pi-process-runtime';
import { ScriptedPiRpcFixture } from '../fixtures/scripted-pi-rpc';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const providerName = 'fixture-provider';
const modelName = 'fixture-model';
const modelId = `${providerName}/${modelName}`;
const invocation = Object.freeze({
  config: Object.freeze({ providerId: 'pi' as const, modelId, thinking: 'high' as const }),
  credential: null,
});

function rpcIdentity(version = PI_RPC_PROTOCOL_VERSION): PiCliIdentity {
  return Object.freeze({
    canonicalPath: process.platform === 'win32' ? 'C:\\opt\\pi.exe' : '/opt/pi',
    packageVersion: version,
    safetyFlags: PI_RPC_REQUIRED_SAFETY_FLAGS,
    fileIdentity: { dev: '1', ino: '2', size: 3, mtimeMs: 4 },
  });
}

function readiness(id: unknown = 'talking-quill-state-1') {
  return {
    id,
    type: 'response',
    command: 'get_state',
    success: true,
    data: {
      model: { provider: providerName, id: modelName },
      thinkingLevel: 'high',
      isStreaming: false,
      isCompacting: false,
      steeringMode: 'all',
      followUpMode: 'one-at-a-time',
      sessionId: 'prepared-session',
      autoCompactionEnabled: true,
      messageCount: 0,
      pendingMessageCount: 0,
    },
  } as const;
}

function assistantMessage(stopReason: 'pending' | 'stop' = 'stop') {
  return {
    role: 'assistant',
    api: 'fixture-api',
    provider: providerName,
    model: modelName,
    content: stopReason === 'pending' ? [] : [{ type: 'text', text: ' cleaned output ' }],
    usage: {},
    stopReason,
    timestamp: 1,
  } as const;
}

function successfulRunRecords(
  promptId: string,
  input = 'private transcript',
): readonly Readonly<Record<string, unknown>>[] {
  const user = { role: 'user', content: input, timestamp: 1 } as const;
  const final = assistantMessage();
  return [
    { id: promptId, type: 'response', command: 'prompt', success: true },
    { type: 'agent_start' },
    { type: 'turn_start' },
    { type: 'message_start', message: user },
    { type: 'message_end', message: user },
    { type: 'message_start', message: assistantMessage('pending') },
    { type: 'message_end', message: final },
    { type: 'turn_end', message: final, toolResults: [] },
    { type: 'agent_end', messages: [user, final] },
    { type: 'agent_settled' },
  ];
}

function emitSuccessfulRun(
  fixture: ScriptedPiRpcFixture,
  promptId: string,
  input = 'private transcript',
): void {
  fixture.sendRecords(successfulRunRecords(promptId, input));
}

function readyFixture(
  onCommand?: (command: Readonly<Record<string, unknown>>, fixture: ScriptedPiRpcFixture) => void,
): ScriptedPiRpcFixture {
  return new ScriptedPiRpcFixture({
    onCommand: (command, fixture) => {
      if (command.type === 'get_state') fixture.send(readiness(command.id));
      if (command.type === 'abort') {
        fixture.send({ id: command.id, type: 'response', command: 'abort', success: true });
      }
      onCommand?.(command, fixture);
    },
  });
}

function printChild(output: string, onClose?: () => void): ChildProcessWithoutNullStreams {
  const child = new EventEmitter() as ChildProcessWithoutNullStreams;
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  Object.assign(child, {
    stdin,
    stdout,
    stderr,
    pid: undefined,
    exitCode: null,
    signalCode: null,
    kill: () => true,
  });
  stdin.once('finish', () => {
    stdout.end(output);
    stderr.end();
    Object.assign(child, { exitCode: 0 });
    child.emit('close', 0, null);
    onClose?.();
  });
  return child;
}

function providerOptions(
  spawnPi: SpawnPi,
  overrides: ConstructorParameters<typeof PiProvider>[0] = {},
) {
  return {
    spawnPi,
    environment: { ...process.env },
    platform: process.platform,
    workingDirectory: process.cwd(),
    resolveCli: () => Promise.resolve(rpcIdentity()),
    revalidateCli: () => Promise.resolve(),
    terminateRpcTree: (child: ChildProcessWithoutNullStreams) => {
      child.emit('close', null, 'SIGKILL');
      return Promise.resolve();
    },
    ...overrides,
  } satisfies ConstructorParameters<typeof PiProvider>[0];
}

describe('PiProvider prepared single-use RPC completion', () => {
  it('observes egress first and launches exact canonical mixed-extension RPC argv with privacy-safe timing', async () => {
    const directory = await createTestDirectory('pi-prepared-extensions');
    try {
      const events: string[] = [];
      const localSource = resolve(directory, 'trusted local.ts');
      const agentDirectory = resolve(directory, 'agent');
      const packageRoot = resolve(agentDirectory, 'npm', 'node_modules', '@trusted', 'extension');
      await mkdir(packageRoot, { recursive: true });
      await writeFile(localSource, 'export default function local() {}', 'utf8');
      await writeFile(
        resolve(packageRoot, 'package.json'),
        JSON.stringify({ name: '@trusted/extension', version: '1.0.0' }),
        'utf8',
      );
      const fixture = readyFixture((command, current) => {
        if (command.type === 'prompt') {
          current.send({
            type: 'extension_ui_request',
            id: 'blocking-confirm',
            method: 'confirm',
            title: 'confirm',
            message: 'continue?',
          });
          emitSuccessfulRun(current, String(command.id));
        }
      });
      const calls: {
        readonly args: readonly string[];
        readonly options: SpawnOptionsWithoutStdio;
      }[] = [];
      const stages: PiRpcTimingStage[] = [];
      const provider = new PiProvider(
        providerOptions(
          (_executable, args, options) => {
            events.push('spawn');
            calls.push({ args, options });
            return fixture.child;
          },
          {
            environment: { ...process.env, PI_CODING_AGENT_DIR: agentDirectory },
            workingDirectory: directory,
            observeEgress: () => events.push('egress'),
            canonicalizeExtensionPath: async (path) => {
              events.push('filesystem');
              return await realpath(path);
            },
            onRpcTiming: (stage) => stages.push(stage),
          },
        ),
      );
      const prepared = await provider.prepareCompletion(
        {
          ...invocation,
          config: {
            ...invocation.config,
            piExtensionSources: [localSource, 'npm:@trusted/extension'],
          },
        },
        new AbortController().signal,
      );
      if (prepared === null) throw new Error('expected prepared completion');

      await expect(
        prepared.complete({ input: 'private transcript' }, new AbortController().signal),
      ).resolves.toBe('cleaned output');
      await prepared.closed;
      const canonicalLocal = await realpath(localSource);
      const canonicalPackage = await realpath(packageRoot);
      expect(events[0]).toBe('egress');
      expect(events.indexOf('filesystem')).toBeGreaterThan(events.indexOf('egress'));
      expect(events.indexOf('spawn')).toBeGreaterThan(events.indexOf('filesystem'));
      expect(calls).toHaveLength(1);
      expect(calls[0]?.args).toEqual([
        '--mode',
        'rpc',
        '--provider',
        providerName,
        '--model',
        modelName,
        '--thinking',
        'high',
        ...PI_RPC_REQUIRED_SAFETY_FLAGS,
        '-e',
        canonicalLocal,
        '-e',
        canonicalPackage,
      ]);
      expect(calls[0]?.options).toMatchObject({ shell: false, windowsHide: true });
      expect(fixture.commands.filter(({ type }) => type === 'extension_ui_response')).toEqual([
        { type: 'extension_ui_response', id: 'blocking-confirm', cancelled: true },
      ]);
      expect(stages).toContain(PiRpcTimingStage.Ready);
      expect(stages.every((stage) => Object.values(PiRpcTimingStage).includes(stage))).toBe(true);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('does not touch extensions, resolve Pi, or spawn when prepared egress observation fails', async () => {
    const canonicalizeExtensionPath = vi.fn(() => Promise.resolve('/trusted/extension.ts'));
    const resolveCli = vi.fn(() => Promise.resolve(rpcIdentity()));
    const spawnPi = vi.fn<SpawnPi>();
    const provider = new PiProvider({
      ...providerOptions(spawnPi),
      observeEgress: () => {
        throw new Error('egress observer failed');
      },
      canonicalizeExtensionPath,
      resolveCli,
    });

    await expect(
      provider.prepareCompletion(
        {
          ...invocation,
          config: { ...invocation.config, piExtensionSources: ['./extension.ts'] },
        },
        new AbortController().signal,
      ),
    ).rejects.toThrow('egress observer failed');
    expect(canonicalizeExtensionPath).not.toHaveBeenCalled();
    expect(resolveCli).not.toHaveBeenCalled();
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it.each([
    ['unsupported version', '0.84.3'],
    ['RPC readiness failure', PI_RPC_PROTOCOL_VERSION],
  ])(
    'falls back exactly once to frozen print mode after %s before prompt write',
    async (_label, version) => {
      const calls: string[][] = [];
      const rpcFixture = new ScriptedPiRpcFixture({
        onCommand: (command, current) => {
          if (command.type === 'get_state')
            current.send({ ...readiness(command.id), success: false });
          if (command.type === 'abort') {
            current.send({ id: command.id, type: 'response', command: 'abort', success: true });
          }
        },
      });
      const mutableCalls = calls;
      const spawnPi: SpawnPi = (_executable, args) => {
        mutableCalls.push([...args]);
        return args.includes('rpc') ? rpcFixture.child : printChild(' print fallback ');
      };
      const provider = new PiProvider(
        providerOptions(spawnPi, { resolveCli: () => Promise.resolve(rpcIdentity(version)) }),
      );
      const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
      if (prepared === null) throw new Error('expected prepared completion');

      await expect(
        prepared.complete({ input: 'only prompt' }, new AbortController().signal),
      ).resolves.toBe('print fallback');
      await prepared.closed;
      expect(mutableCalls.filter((args) => args.includes('rpc'))).toHaveLength(
        version === PI_RPC_PROTOCOL_VERSION ? 1 : 0,
      );
      expect(mutableCalls.filter((args) => args.includes('-p'))).toEqual([
        ['-p', '--model', modelId, '--thinking', 'high', ...PI_RPC_REQUIRED_SAFETY_FLAGS],
      ]);
    },
  );

  it('re-resolves an executable that changes before the prepared RPC spawn', async () => {
    const first = Object.freeze({ ...rpcIdentity(), canonicalPath: 'first-pi' });
    const second = Object.freeze({ ...rpcIdentity(), canonicalPath: 'second-pi' });
    const resolveCli = vi
      .fn<() => Promise<PiCliIdentity>>()
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second);
    const revalidateCli = vi.fn((candidate: PiCliIdentity) =>
      candidate.canonicalPath === first.canonicalPath
        ? Promise.reject(new Error('changed'))
        : Promise.resolve(),
    );
    const fixture = readyFixture((command, current) => {
      if (command.type === 'prompt') emitSuccessfulRun(current, String(command.id));
    });
    const executables: string[] = [];
    const provider = new PiProvider(
      providerOptions(
        (executable) => {
          executables.push(executable);
          return fixture.child;
        },
        { resolveCli, revalidateCli },
      ),
    );

    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');
    await expect(
      prepared.complete({ input: 'private transcript' }, new AbortController().signal),
    ).resolves.toBe('cleaned output');
    await prepared.closed;
    expect(resolveCli).toHaveBeenCalledTimes(2);
    expect(executables).toEqual(['second-pi']);
  });

  it('never falls back or duplicates after prompt bytes may have been written', async () => {
    const fixture = readyFixture((command, current) => {
      if (command.type === 'prompt') current.send({ type: 'unexpected-after-prompt' });
    });
    const calls: string[][] = [];
    const provider = new PiProvider(
      providerOptions((_executable, args) => {
        calls.push([...args]);
        return fixture.child;
      }),
    );
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');

    await expect(
      prepared.complete({ input: 'single delivery' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE', fallbackEligible: false });
    await expect(prepared.closed).resolves.toBeUndefined();
    expect(calls).toHaveLength(1);
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('does not cross the provider boundary for duplicate settlement in one stdout chunk', async () => {
    const fixture = readyFixture((command, current) => {
      if (command.type !== 'prompt') return;
      current.sendRecords([
        ...successfulRunRecords(String(command.id), 'single delivery'),
        { type: 'agent_settled' },
      ]);
    });
    const calls: string[][] = [];
    const provider = new PiProvider(
      providerOptions((_executable, args) => {
        calls.push([...args]);
        return fixture.child;
      }),
    );
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');

    await expect(
      prepared.complete({ input: 'single delivery' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE', fallbackEligible: false });
    await expect(prepared.closed).resolves.toBeUndefined();
    expect(calls).toHaveLength(1);
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('revokes an unused speculative child for foreground work without overlapping children', async () => {
    const rpcFixture = readyFixture();
    let active = 0;
    let maxActive = 0;
    const calls: string[][] = [];
    const spawnPi: SpawnPi = (_executable, args) => {
      calls.push([...args]);
      active += 1;
      maxActive = Math.max(maxActive, active);
      if (args.includes('rpc')) {
        rpcFixture.child.once('close', () => {
          active -= 1;
        });
        return rpcFixture.child;
      }
      return printChild(
        'provider  model  context  max-out  thinking  images\nfixture-provider  fixture-model  8K  1K  yes  no\n',
        () => {
          active -= 1;
        },
      );
    };
    const provider = new PiProvider(providerOptions(spawnPi, { rpcReadyTtlMs: 2_000 }));
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');

    await expect(provider.listModels(invocation, new AbortController().signal)).resolves.toEqual([
      expect.objectContaining({ id: modelId }),
    ]);
    expect(maxActive).toBe(1);
    expect(calls).toHaveLength(2);
    prepared.requestClose('unused');
    await prepared.closed;
  });

  it('retires speculation before probing a changed executable for foreground work', async () => {
    let configuredPath: string | null = 'first-path';
    const events: string[] = [];
    const rpcFixture = readyFixture();
    rpcFixture.child.once('close', () => events.push('rpc-closed'));
    const resolveCli = vi.fn(() => {
      events.push(`resolve:${String(configuredPath)}`);
      return Promise.resolve(
        Object.freeze({ ...rpcIdentity(), canonicalPath: configuredPath ?? 'discovered-pi' }),
      );
    });
    const provider = new PiProvider(
      providerOptions(
        (_executable, args) =>
          args.includes('rpc')
            ? rpcFixture.child
            : printChild(
                'provider  model  context  max-out  thinking  images\nfixture-provider  fixture-model  8K  1K  yes  no\n',
              ),
        { configuredPath: () => configuredPath, resolveCli },
      ),
    );
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');
    configuredPath = 'second-path';

    await provider.listModels(invocation, new AbortController().signal);
    expect(events).toEqual(['resolve:first-path', 'rpc-closed', 'resolve:second-path']);
    prepared.requestClose('superseded');
    await prepared.closed;
  });

  it('does not revoke a prompt-committed operation and queues foreground work until retirement', async () => {
    let promptId = '';
    const rpcFixture = readyFixture((command) => {
      if (command.type === 'prompt') promptId = String(command.id);
    });
    const calls: string[][] = [];
    const provider = new PiProvider(
      providerOptions((_executable, args) => {
        calls.push([...args]);
        return args.includes('rpc')
          ? rpcFixture.child
          : printChild(
              'provider  model  context  max-out  thinking  images\nfixture-provider  fixture-model  8K  1K  yes  no\n',
            );
      }),
    );
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');
    const completion = prepared.complete(
      { input: 'private transcript' },
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(promptId).not.toBe(''));
    const foreground = provider.listModels(invocation, new AbortController().signal);
    await new Promise<void>((resolveWait) => setTimeout(resolveWait, 20));
    expect(calls).toHaveLength(1);

    emitSuccessfulRun(rpcFixture, promptId);
    await expect(completion).resolves.toBe('cleaned output');
    await expect(foreground).resolves.toEqual([expect.objectContaining({ id: modelId })]);
    await prepared.closed;
    expect(calls).toHaveLength(2);
    expect(rpcFixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('retires without prompting when configuration or resource identity changes before use', async () => {
    const directory = await createTestDirectory('pi-prepared-resource-change');
    try {
      const extension = resolve(directory, 'extension.ts');
      await writeFile(extension, 'export default function first() {}', 'utf8');
      for (const changed of ['extension', 'executable', 'configuration'] as const) {
        let revalidations = 0;
        let configuredPath: string | null = 'first-pi-path';
        const fixture = readyFixture();
        const provider = new PiProvider(
          providerOptions(() => fixture.child, {
            workingDirectory: directory,
            configuredPath: () => configuredPath,
            revalidateCli: () => {
              revalidations += 1;
              return changed === 'executable' && revalidations >= 2
                ? Promise.reject(new Error('changed'))
                : Promise.resolve();
            },
          }),
        );
        const prepared = await provider.prepareCompletion(
          {
            ...invocation,
            config: { ...invocation.config, piExtensionSources: [extension] },
          },
          new AbortController().signal,
        );
        if (prepared === null) throw new Error('expected prepared completion');
        if (changed === 'extension') {
          await writeFile(extension, 'export default function changedAndLonger() {}', 'utf8');
        }
        if (changed === 'configuration') configuredPath = 'second-pi-path';
        await expect(
          prepared.complete({ input: 'must not dispatch' }, new AbortController().signal),
        ).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
        await prepared.closed;
        expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(0);
      }
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('retires when installed npm package implementation metadata changes before use', async () => {
    const directory = await createTestDirectory('pi-prepared-npm-resource-change');
    try {
      const agentDirectory = resolve(directory, 'agent');
      const packageRoot = resolve(agentDirectory, 'npm', 'node_modules', 'prepared-extension');
      const extensionDirectory = resolve(packageRoot, 'extensions');
      const extensionFile = resolve(extensionDirectory, 'index.ts');
      await mkdir(extensionDirectory, { recursive: true });
      await writeFile(
        resolve(packageRoot, 'package.json'),
        JSON.stringify({
          name: 'prepared-extension',
          version: '1.0.0',
          pi: { extensions: ['./extensions'] },
        }),
        'utf8',
      );
      await writeFile(extensionFile, 'export default function first() {}', 'utf8');
      const fixture = readyFixture();
      const provider = new PiProvider(
        providerOptions(() => fixture.child, {
          environment: { ...process.env, PI_CODING_AGENT_DIR: agentDirectory },
          workingDirectory: directory,
        }),
      );
      const prepared = await provider.prepareCompletion(
        {
          ...invocation,
          config: { ...invocation.config, piExtensionSources: ['npm:prepared-extension'] },
        },
        new AbortController().signal,
      );
      if (prepared === null) throw new Error('expected prepared completion');
      await writeFile(extensionFile, 'export default function changedAndLonger() {}', 'utf8');

      await expect(
        prepared.complete({ input: 'must not dispatch' }, new AbortController().signal),
      ).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
      await prepared.closed;
      expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(0);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it.each([
    ['model mismatch', { input: 'must not dispatch', modelId: 'other/model' }, false],
    ['cancellation', { input: 'must not dispatch' }, true],
  ] as const)('retires before prompt on %s', async (_label, request, cancel) => {
    const fixture = readyFixture();
    const provider = new PiProvider(providerOptions(() => fixture.child));
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');
    const controller = new AbortController();
    if (cancel) controller.abort();

    await expect(prepared.complete(request, controller.signal)).rejects.toMatchObject({
      code: cancel ? 'CANCELLED' : 'INVALID_CONFIG',
      fallbackEligible: true,
    });
    await prepared.closed;
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(0);
  });

  it('expires an unused ready lease and confirms cleanup before closing', async () => {
    const fixture = readyFixture();
    const provider = new PiProvider(providerOptions(() => fixture.child, { rpcReadyTtlMs: 20 }));
    const prepared = await provider.prepareCompletion(invocation, new AbortController().signal);
    if (prepared === null) throw new Error('expected prepared completion');

    await expect(prepared.closed).resolves.toBeUndefined();
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(0);
    await expect(
      prepared.complete({ input: 'too late' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
  });
});
