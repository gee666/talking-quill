import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { EventEmitter } from 'node:events';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import { PiProvider, parsePiModels } from '../../app/src/main/providers/pi';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';
import {
  automaticCandidates,
  piSpawnCommand,
  windowsSystemTools,
  type PiCliIdentity,
} from '../../app/src/main/providers/pi-discovery';

const PI_ISOLATION_FLAGS = [
  '--no-tools',
  '--no-session',
  '--no-context-files',
  '--no-approve',
  '--no-skills',
  '--no-prompt-templates',
  '--no-themes',
  '--offline',
] as const;

const identity: PiCliIdentity = Object.freeze({
  canonicalPath: 'C:\\Users\\Example User\\AppData\\Local\\pnpm\\Pi.CMD',
  packageVersion: '99.0.0-future',
  safetyFlags: PI_ISOLATION_FLAGS,
  fileIdentity: { dev: '1', ino: '1', size: 1, mtimeMs: 1 },
});

describe('external Pi adapter', () => {
  it('scans Windows PATH and PATHEXT case-insensitively with native shims before extensionless files', async () => {
    const candidates = await automaticCandidates(
      { PaTh: 'C:\\Program Files\\Pi Bin', PaThExT: '.cMd;.EXE' },
      'win32',
    );
    const pathCandidates = candidates.filter(({ source }) => source === 'path');
    expect(pathCandidates.slice(0, 3).map(({ path }) => path)).toEqual([
      'C:\\Program Files\\Pi Bin\\pi.cmd',
      'C:\\Program Files\\Pi Bin\\pi.exe',
      'C:\\Program Files\\Pi Bin\\pi',
    ]);
  });
  it('uses only the fixed cmd bridge for Windows shims', () => {
    expect(
      piSpawnCommand(
        identity.canonicalPath,
        ['-p', '--model', 'future/model-v2', '--thinking', 'xhigh', '--no-tools'],
        {
          SystemRoot: 'C:\\Windows',
          ComSpec: 'C:\\Windows\\System32\\cmd.exe',
        },
        'win32',
      ),
    ).toEqual({
      executable: 'C:\\Windows\\System32\\cmd.exe',
      args: [
        '/d',
        '/s',
        '/c',
        '""C:\\Users\\Example User\\AppData\\Local\\pnpm\\Pi.CMD" -p --model future/model-v2 --thinking xhigh --no-tools"',
      ],
    });
    expect(() =>
      piSpawnCommand(
        identity.canonicalPath,
        ['--model', 'p/model&calc'],
        { SystemRoot: 'C:\\Windows' },
        'win32',
      ),
    ).toThrow();
  });

  it('derives canonical System32 tools from mixed-case Windows environment keys', () => {
    expect(
      windowsSystemTools({
        sYsTeMrOoT: 'D:/Windows',
        cOmSpEc: 'd:\\windows\\SYSTEM32\\CMD.EXE',
      }),
    ).toEqual({
      systemRoot: 'D:\\Windows',
      system32: 'D:\\Windows\\System32',
      where: 'D:\\Windows\\System32\\where.exe',
      cmd: 'D:\\Windows\\System32\\cmd.exe',
      taskkill: 'D:\\Windows\\System32\\taskkill.exe',
    });
    expect(() =>
      windowsSystemTools({ SystemRoot: 'D:\\Windows', ComSpec: 'C:\\hostile\\cmd.exe' }),
    ).toThrow();
    expect(() => windowsSystemTools({ SystemRoot: '\\\\server\\Windows' })).toThrow();
    expect(() =>
      windowsSystemTools({ SystemRoot: 'D:\\Windows', SYSTEMROOT: 'C:\\Windows' }),
    ).toThrow();
  });

  it('parses changed spacing and columns without provider filtering', () => {
    expect(
      parsePiModels(
        'PROVIDER   MODEL   CONTEXT   EXTRA   IMAGES\nfuture-cloud   alpha-2   256K   value   yes\nlocal-x\tbeta\t8K\tvalue\tno\n',
      ).map(({ id }) => id),
    ).toEqual(['future-cloud/alpha-2', 'local-x/beta']);
    expect(parsePiModels('future output not understood')).toEqual([]);
  });

  it('Test Connection lists models then invokes the exact selected model with a fixed stdin prompt', async () => {
    const calls: { args: readonly string[]; input: string }[] = [];
    const spawnPi = vi.fn((_executable: string, args: readonly string[]) => {
      const child = new EventEmitter() as ChildProcessWithoutNullStreams;
      const stdin = new PassThrough();
      const stdout = new PassThrough();
      const stderr = new PassThrough();
      Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
      const chunks: Buffer[] = [];
      stdin.on('data', (chunk: Buffer) => chunks.push(Buffer.from(chunk)));
      stdin.on('finish', () => {
        calls.push({ args, input: Buffer.concat(chunks).toString('utf8') });
        stdout.end(
          args.includes('--list-models')
            ? 'provider  model  context  max-out  thinking  images\nfuture  model-v2  8K  1K  yes  no\n'
            : 'TALKING_QUILL_CONNECTION_OK',
        );
        stderr.end();
        child.emit('close', 0, null);
      });
      return child;
    });
    const observeEgress = vi.fn();
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      observeEgress,
      resolveCli: () => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }),
    });
    await expect(
      provider.validate(
        {
          config: { providerId: 'pi', modelId: 'future/model-v2', thinking: 'high' },
          credential: null,
        },
        new AbortController().signal,
      ),
    ).resolves.toMatchObject({ ok: true, modelCount: 1 });
    expect(calls).toHaveLength(2);
    expect(calls[0]?.args).toEqual(['--list-models', ...PI_ISOLATION_FLAGS]);
    expect(calls[1]).toEqual({
      args: ['-p', '--model', 'future/model-v2', '--thinking', 'high', ...PI_ISOLATION_FLAGS],
      input: 'Reply with exactly: TALKING_QUILL_CONNECTION_OK',
    });
    expect(observeEgress).toHaveBeenCalledTimes(1);
  });

  it('does not settle cancellation until a real hung root and descendant are gone', async () => {
    const directory = await createTestDirectory('pi-tree');
    const receipt = resolve(directory, 'pids.json');
    try {
      const provider = new PiProvider({
        platform: process.platform,
        environment: { ...process.env, TALKING_QUILL_PI_TREE_RECEIPT: receipt },
        resolveCli: () =>
          Promise.resolve({ ...identity, canonicalPath: process.execPath, safetyFlags: [] }),
        spawnPi: (_executable, _args, options) =>
          spawn(process.execPath, [resolve('tests/fixtures/pi-hung-tree.cjs')], options),
      });
      const controller = new AbortController();
      const completion = provider.cleanTranscript(
        {
          config: { providerId: 'pi', modelId: 'future/model-v2', thinking: 'high' },
          credential: null,
        },
        { input: 'bounded prompt' },
        controller.signal,
      );
      let pids: { root: number; descendant: number } | undefined;
      const deadline = Date.now() + 5_000;
      while (pids === undefined && Date.now() < deadline) {
        try {
          pids = JSON.parse(await readFile(receipt, 'utf8')) as typeof pids;
        } catch {
          await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
        }
      }
      expect(pids).toBeDefined();
      controller.abort();
      await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
      expect(processExists(pids?.root ?? -1)).toBe(false);
      expect(processExists(pids?.descendant ?? -1)).toBe(false);
    } finally {
      await removeTestDirectory(directory);
    }
  }, 15_000);

  it('cancels a running Pi process and settles without waiting for output', async () => {
    const kill = vi.fn(() => true);
    const spawnPi = vi.fn(() => {
      const child = new EventEmitter() as ChildProcessWithoutNullStreams;
      Object.assign(child, {
        stdin: new PassThrough(),
        stdout: new PassThrough(),
        stderr: new PassThrough(),
        pid: undefined,
        kill,
      });
      return child;
    });
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      resolveCli: () =>
        Promise.resolve({ ...identity, canonicalPath: '/opt/pi', safetyFlags: ['--no-tools'] }),
    });
    const controller = new AbortController();
    const completion = provider.cleanTranscript(
      {
        config: { providerId: 'pi', modelId: 'future/model-v2', thinking: 'high' },
        credential: null,
      },
      { input: 'bounded prompt' },
      controller.signal,
    );
    await new Promise<void>((resolveWait) => setTimeout(resolveWait, 0));
    controller.abort();
    await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(kill).toHaveBeenCalledWith('SIGKILL');
  });

  it('reprobes and replaces a cached executable identity before runtime', async () => {
    const replacement = {
      ...identity,
      packageVersion: '100.0.0-replacement',
      fileIdentity: { ...identity.fileIdentity, size: 2, mtimeMs: 2 },
    };
    const resolveCli = vi
      .fn<() => Promise<PiCliIdentity>>()
      .mockResolvedValueOnce(identity)
      .mockResolvedValue(replacement);
    let staleChecks = 0;
    const revalidateCli = vi.fn((value: PiCliIdentity) => {
      if (value === identity && staleChecks++ > 0) return Promise.reject(new Error('replaced'));
      return Promise.resolve();
    });
    const spawnPi = vi.fn(() => {
      const child = new EventEmitter() as ChildProcessWithoutNullStreams;
      const stdin = new PassThrough();
      const stdout = new PassThrough();
      const stderr = new PassThrough();
      Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
      stdin.on('finish', () => {
        stdout.end('cleaned');
        stderr.end();
        child.emit('close', 0, null);
      });
      return child;
    });
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      resolveCli,
      revalidateCli,
    });
    const invocation = {
      config: { providerId: 'pi' as const, modelId: 'future/model-v2', thinking: 'high' as const },
      credential: null,
    };
    await provider.cleanTranscript(invocation, { input: 'one' }, new AbortController().signal);
    await provider.cleanTranscript(invocation, { input: 'two' }, new AbortController().signal);
    expect(resolveCli).toHaveBeenCalledTimes(2);
    expect(revalidateCli).toHaveBeenLastCalledWith(replacement, expect.any(AbortSignal));
  });

  it('passes exact model/thinking argv, bounded prompt stdin, normal env, and safety flags', async () => {
    const calls: {
      executable: string;
      args: readonly string[];
      input: string;
      env: NodeJS.ProcessEnv;
    }[] = [];
    const spawnPi = vi.fn(
      (executable: string, args: readonly string[], options: SpawnOptionsWithoutStdio) => {
        const child = new EventEmitter() as ChildProcessWithoutNullStreams;
        const stdin = new PassThrough();
        const stdout = new PassThrough();
        const stderr = new PassThrough();
        Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
        const chunks: Buffer[] = [];
        stdin.on('data', (chunk: Buffer) => chunks.push(Buffer.from(chunk)));
        stdin.on('finish', () => {
          calls.push({
            executable,
            args,
            input: Buffer.concat(chunks).toString('utf8'),
            env: options.env ?? {},
          });
          stdout.end('cleaned words');
          stderr.end();
          child.emit('close', 0, null);
        });
        return child;
      },
    );
    const provider = new PiProvider({
      spawnPi,
      platform: 'win32',
      environment: {
        SystemRoot: 'C:\\Windows',
        ComSpec: 'C:\\Windows\\System32\\cmd.exe',
        PI_CODING_AGENT_DIR: 'C:\\Users\\me\\.pi-custom',
      },
      workingDirectory: 'C:\\neutral app data',
      resolveCli: () => Promise.resolve(identity),
    });
    const output = await provider.cleanTranscript(
      {
        config: { providerId: 'pi', modelId: 'future/model-v2', thinking: 'xhigh' },
        credential: null,
      },
      { input: 'prompt & never shell-expanded' },
      new AbortController().signal,
    );
    expect(output).toBe('cleaned words');
    expect(calls).toHaveLength(1);
    expect(calls[0]?.args[3]).toBe(
      `""C:\\Users\\Example User\\AppData\\Local\\pnpm\\Pi.CMD" -p --model future/model-v2 --thinking xhigh ${PI_ISOLATION_FLAGS.join(' ')}"`,
    );
    expect(calls[0]?.input).toBe('prompt & never shell-expanded');
    expect(calls[0]?.env.PI_CODING_AGENT_DIR).toBe('C:\\Users\\me\\.pi-custom');
  });
});

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}
