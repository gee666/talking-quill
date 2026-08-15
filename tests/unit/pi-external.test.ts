import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { EventEmitter } from 'node:events';
import type { Stats } from 'node:fs';
import { chmod, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { PassThrough } from 'node:stream';
import { describe, expect, it, vi } from 'vitest';
import { PiProvider, parsePiModels } from '../../app/src/main/providers/pi';
import {
  PI_MIN_OPERATION_TIMEOUT_MS,
  ProviderService,
} from '../../app/src/main/providers/provider-service';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';
import {
  automaticCandidates,
  piSpawnCommand,
  validatePiExecutable,
  windowsSystemTools,
  type PiCliIdentity,
} from '../../app/src/main/providers/pi-discovery';

const PI_ISOLATION_FLAGS = [
  '--no-tools',
  '--no-extensions',
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
    expect(
      piSpawnCommand(
        identity.canonicalPath,
        ['--no-extensions', '-e', 'C:\\Trusted Extensions\\cleanup.ts'],
        { SystemRoot: 'C:\\Windows' },
        'win32',
      ).args[3],
    ).toBe(
      '""C:\\Users\\Example User\\AppData\\Local\\pnpm\\Pi.CMD" --no-extensions -e "C:\\Trusted Extensions\\cleanup.ts""',
    );
    for (const source of [
      'C:\\safe&calc\\extension.ts',
      'C:\\%TEMP%\\extension.ts',
      'C:\\!PROMPT!\\extension.ts',
      'C:\\safe^(calc^)\\extension.ts',
      'C:\\safe|calc\\extension.ts',
      'C:\\safe>stolen.txt',
    ]) {
      expect(() =>
        piSpawnCommand(
          identity.canonicalPath,
          ['-e', source],
          { SystemRoot: 'C:\\Windows' },
          'win32',
        ),
      ).toThrow();
    }
  });

  it('requires isolated extension discovery and a repeatable explicit extension option', async () => {
    const directory = await createTestDirectory('pi-capabilities');
    try {
      const compatible = await writePiCapabilityFixture(directory, 'compatible', true);
      const validated = await validatePiExecutable(compatible, process.env, process.platform);
      expect(validated.safetyFlags).toEqual(PI_ISOLATION_FLAGS);

      const nonRepeatable = await writePiCapabilityFixture(directory, 'non-repeatable', false);
      await expect(
        validatePiExecutable(nonRepeatable, process.env, process.platform),
      ).rejects.toMatchObject({ code: 'PI_INCOMPATIBLE' });
    } finally {
      await removeTestDirectory(directory);
    }
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
          config: {
            providerId: 'pi',
            modelId: 'future/model-v2',
            thinking: 'high',
            piExtensionSources: [
              'tests/fixtures/pi-hung-tree.cjs',
              'tests/fixtures/validate-provider-logos.cjs',
            ],
          },
          credential: null,
        },
        new AbortController().signal,
      ),
    ).resolves.toMatchObject({ ok: true, modelCount: 1 });
    expect(calls).toHaveLength(2);
    const extensionArgs = [
      '-e',
      'tests/fixtures/pi-hung-tree.cjs',
      '-e',
      'tests/fixtures/validate-provider-logos.cjs',
    ];
    expect(calls[0]?.args).toEqual(['--list-models', ...PI_ISOLATION_FLAGS, ...extensionArgs]);
    expect(calls[1]).toEqual({
      args: [
        '-p',
        '--model',
        'future/model-v2',
        '--thinking',
        'high',
        ...PI_ISOLATION_FLAGS,
        ...extensionArgs,
      ],
      input: 'Reply with exactly: TALKING_QUILL_CONNECTION_OK',
    });
    await expect(
      provider.cleanTranscript(
        {
          config: {
            providerId: 'pi',
            modelId: 'future/model-v2',
            thinking: 'high',
            piExtensionSources: [
              'tests/fixtures/pi-hung-tree.cjs',
              'tests/fixtures/validate-provider-logos.cjs',
            ],
          },
          credential: null,
        },
        { input: 'configured completion' },
        new AbortController().signal,
      ),
    ).resolves.toBe('TALKING_QUILL_CONNECTION_OK');
    expect(calls[2]).toEqual({
      args: [
        '-p',
        '--model',
        'future/model-v2',
        '--thinking',
        'high',
        ...PI_ISOLATION_FLAGS,
        ...extensionArgs,
      ],
      input: 'configured completion',
    });
    expect(observeEgress).toHaveBeenCalledTimes(2);
  });

  it('does not spawn Pi when egress observation rejects an extension-capable validation', async () => {
    const observerError = new Error('egress audit unavailable');
    const spawnPi = vi.fn();
    const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
    const canonicalizeExtensionPath = vi.fn(() =>
      Promise.resolve('/trusted/extensions/cleanup.ts'),
    );
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      observeEgress: () => {
        throw observerError;
      },
      resolveCli,
      canonicalizeExtensionPath,
    });

    await expect(
      provider.validate(
        {
          config: {
            providerId: 'pi',
            modelId: 'future/model-v2',
            thinking: 'off',
            piExtensionSources: ['tests/fixtures/pi-hung-tree.cjs'],
          },
          credential: null,
        },
        new AbortController().signal,
      ),
    ).rejects.toBe(observerError);
    expect(canonicalizeExtensionPath).not.toHaveBeenCalled();
    expect(resolveCli).not.toHaveBeenCalled();
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it('rejects configured UNC roots before filesystem resolution', async () => {
    for (const source of [
      '\\\\server\\share\\extension.ts',
      '//server/share/extension.ts',
      '\\\\?\\UNC\\server\\share\\extension.ts',
    ]) {
      const canonicalizeExtensionPath = vi.fn(() => Promise.resolve(source));
      const observeEgress = vi.fn();
      const provider = new PiProvider({
        platform: 'win32',
        canonicalizeExtensionPath,
        observeEgress,
      });

      await expect(
        provider.listModels(
          { config: { providerId: 'pi', piExtensionSources: [source] }, credential: null },
          new AbortController().signal,
        ),
      ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
      expect(canonicalizeExtensionPath).not.toHaveBeenCalled();
      expect(observeEgress).not.toHaveBeenCalled();
    }
  });

  it('observes egress before rejecting local paths that canonicalize to network roots', async () => {
    for (const canonicalPath of [
      '\\\\server\\share\\extension.ts',
      '\\\\?\\UNC\\server\\share\\extension.ts',
    ]) {
      const events: string[] = [];
      const spawnPi = vi.fn();
      const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
      const provider = new PiProvider({
        spawnPi,
        platform: 'win32',
        workingDirectory: 'C:\\Talking Quill',
        observeEgress: () => events.push('egress'),
        canonicalizeExtensionPath: () => {
          events.push('realpath');
          return Promise.resolve(canonicalPath);
        },
        resolveCli,
      });

      await expect(
        provider.listModels(
          {
            config: { providerId: 'pi', piExtensionSources: ['./local-link.ts'] },
            credential: null,
          },
          new AbortController().signal,
        ),
      ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
      expect(events).toEqual(['egress', 'realpath']);
      expect(resolveCli).not.toHaveBeenCalled();
      expect(spawnPi).not.toHaveBeenCalled();
    }
  });

  it('resolves installed npm opt-ins from the Pi agent directory and invalidates their model cache identity', async () => {
    const directory = await createTestDirectory('pi-installed-extension');
    try {
      const agentDirectory = resolve(directory, 'Pi Agent');
      const packageName = '@trusted/installed-extension';
      const packageRoot = await writeInstalledPiExtensionPackage(
        agentDirectory,
        packageName,
        '1.0.0',
      );
      await writeFile(
        resolve(directory, 'local extension.ts'),
        'export default function local() {}',
      );
      const calls: string[][] = [];
      const spawnPi = modelListSpawn(calls);
      const provider = new PiProvider({
        spawnPi,
        environment: { ...process.env, PI_CODING_AGENT_DIR: agentDirectory },
        platform: process.platform,
        workingDirectory: directory,
        resolveCli: () => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }),
      });
      const invocation = {
        config: {
          providerId: 'pi' as const,
          piExtensionSources: ['./local extension.ts', `npm:${packageName}`],
        },
        credential: null,
      };
      const signal = new AbortController().signal;

      await provider.listModels(invocation, signal);
      await provider.listModels(invocation, signal);
      const canonicalPackageRoot = await realpath(packageRoot);
      expect(calls).toEqual([
        [
          '--list-models',
          ...PI_ISOLATION_FLAGS,
          '-e',
          './local extension.ts',
          '-e',
          canonicalPackageRoot,
        ],
      ]);
      expect(calls[0]).not.toContain(`npm:${packageName}`);

      await writeInstalledPiExtensionPackage(agentDirectory, packageName, '1.0.1');
      await provider.listModels(invocation, signal);
      expect(calls).toHaveLength(2);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it("uses Pi's default agent directory for installed npm opt-ins", async () => {
    const directory = await createTestDirectory('pi-default-agent-extension');
    try {
      const home = resolve(directory, 'home');
      const agentDirectory = resolve(home, '.pi', 'agent');
      const packageName = 'installed-extension';
      const packageRoot = await writeInstalledPiExtensionPackage(
        agentDirectory,
        packageName,
        '2.0.0',
      );
      const calls: string[][] = [];
      const provider = new PiProvider({
        spawnPi: modelListSpawn(calls),
        environment: process.platform === 'win32' ? { USERPROFILE: home } : { HOME: home },
        platform: process.platform,
        workingDirectory: directory,
        resolveCli: () => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }),
      });

      await provider.listModels(
        {
          config: { providerId: 'pi', piExtensionSources: [`npm:${packageName}`] },
          credential: null,
        },
        new AbortController().signal,
      );
      expect(calls[0]).toEqual([
        '--list-models',
        ...PI_ISOLATION_FLAGS,
        '-e',
        await realpath(packageRoot),
      ]);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('rejects missing and mismatched installed npm packages before resolving or spawning Pi', async () => {
    const directory = await createTestDirectory('pi-invalid-installed-extension');
    try {
      const agentDirectory = resolve(directory, 'agent');
      await writeInstalledPiExtensionPackage(
        agentDirectory,
        'mismatched-extension',
        '1.0.0',
        'different-package-name',
      );
      for (const packageName of ['missing-extension', 'mismatched-extension']) {
        const spawnPi = vi.fn();
        const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
        const provider = new PiProvider({
          spawnPi,
          environment: { ...process.env, PI_CODING_AGENT_DIR: agentDirectory },
          platform: process.platform,
          workingDirectory: directory,
          resolveCli,
        });

        await expect(
          provider.listModels(
            {
              config: { providerId: 'pi', piExtensionSources: [`npm:${packageName}`] },
              credential: null,
            },
            new AbortController().signal,
          ),
        ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
        expect(resolveCli).not.toHaveBeenCalled();
        expect(spawnPi).not.toHaveBeenCalled();
      }
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('bounds a never-settling installed package manifest read without spawning Pi', async () => {
    const directory = await createTestDirectory('pi-package-read-timeout');
    try {
      const agentDirectory = resolve(directory, 'agent');
      await writeInstalledPiExtensionPackage(agentDirectory, 'installed-extension', '1.0.0');
      const spawnPi = vi.fn();
      const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
      const provider = new PiProvider({
        spawnPi,
        environment: { ...process.env, PI_CODING_AGENT_DIR: agentDirectory },
        platform: process.platform,
        extensionResolutionTimeoutMs: 25,
        readExtensionFile: () => new Promise<never>(() => undefined),
        resolveCli,
      });

      await expect(
        provider.listModels(
          {
            config: {
              providerId: 'pi',
              piExtensionSources: ['npm:installed-extension'],
            },
            credential: null,
          },
          new AbortController().signal,
        ),
      ).rejects.toMatchObject({ code: 'TIMEOUT' });
      expect(resolveCli).not.toHaveBeenCalled();
      expect(spawnPi).not.toHaveBeenCalled();
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it.each(['realpath', 'stat', 'access'] as const)(
    'bounds a never-settling extension %s operation without spawning Pi',
    async (stage) => {
      const never = new Promise<never>(() => undefined);
      const spawnPi = vi.fn();
      const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
      const provider = new PiProvider({
        spawnPi,
        platform: 'linux',
        extensionResolutionTimeoutMs: 25,
        canonicalizeExtensionPath: () =>
          stage === 'realpath' ? never : Promise.resolve('/trusted/extension.ts'),
        statExtensionPath: () =>
          stage === 'stat' ? never : Promise.resolve(regularExtensionStats()),
        accessExtensionPath: () => (stage === 'access' ? never : Promise.resolve()),
        resolveCli,
      });
      const startedAt = Date.now();

      await expect(
        provider.cleanTranscript(
          {
            config: {
              providerId: 'pi',
              modelId: 'future/model-v2',
              thinking: 'off',
              piExtensionSources: ['./extension.ts'],
            },
            credential: null,
          },
          { input: 'bounded prompt' },
          new AbortController().signal,
        ),
      ).rejects.toMatchObject({ code: 'TIMEOUT' });
      expect(Date.now() - startedAt).toBeLessThan(1_000);
      expect(resolveCli).not.toHaveBeenCalled();
      expect(spawnPi).not.toHaveBeenCalled();
    },
  );

  it('cancels extension resolution and ignores late filesystem completion', async () => {
    let completeRealpath!: (path: string) => void;
    const realpathPending = new Promise<string>((resolveRealpath) => {
      completeRealpath = resolveRealpath;
    });
    const canonicalizeExtensionPath = vi.fn(() => realpathPending);
    const statExtensionPath = vi.fn(() => Promise.resolve(regularExtensionStats()));
    const spawnPi = vi.fn();
    const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      extensionResolutionTimeoutMs: 10_000,
      canonicalizeExtensionPath,
      statExtensionPath,
      resolveCli,
    });
    const controller = new AbortController();
    const completion = provider.cleanTranscript(
      {
        config: {
          providerId: 'pi',
          modelId: 'future/model-v2',
          thinking: 'off',
          piExtensionSources: ['./extension.ts'],
        },
        credential: null,
      },
      { input: 'cancelled prompt' },
      controller.signal,
    );
    await vi.waitFor(() => expect(canonicalizeExtensionPath).toHaveBeenCalledOnce());
    controller.abort();

    await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
    completeRealpath('/trusted/extension.ts');
    await new Promise<void>((resolveTurn) => setTimeout(resolveTurn, 0));
    expect(statExtensionPath).not.toHaveBeenCalled();
    expect(resolveCli).not.toHaveBeenCalled();
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it('settles at the ProviderService Pi deadline when extension realpath never settles', async () => {
    const spawnPi = vi.fn();
    const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
    const service = new ProviderService(
      new ProviderRegistry({
        pi: {
          spawnPi,
          platform: 'linux',
          extensionResolutionTimeoutMs: 60_000,
          canonicalizeExtensionPath: () => new Promise<never>(() => undefined),
          resolveCli,
        },
      }),
      { getCredential: () => null },
      { operationTimeoutMs: PI_MIN_OPERATION_TIMEOUT_MS },
    );
    const startedAt = Date.now();

    await expect(
      service.cleanTranscript(
        {
          providerId: 'pi',
          modelId: 'future/model-v2',
          thinking: 'off',
          piExtensionSources: ['./extension.ts'],
        },
        { input: 'raw fallback input' },
        new AbortController().signal,
      ),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });
    expect(Date.now() - startedAt).toBeLessThan(2_000);
    expect(resolveCli).not.toHaveBeenCalled();
    expect(spawnPi).not.toHaveBeenCalled();
    service.dispose();
  });

  it('rejects missing local extension files before resolving or spawning Pi', async () => {
    const spawnPi = vi.fn();
    const resolveCli = vi.fn(() => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }));
    const provider = new PiProvider({ spawnPi, platform: 'linux', resolveCli });

    await expect(
      provider.listModels(
        {
          config: { providerId: 'pi', piExtensionSources: ['./missing-extension.ts'] },
          credential: null,
        },
        new AbortController().signal,
      ),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    expect(resolveCli).not.toHaveBeenCalled();
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it('defaults to no explicit extensions and keys the model cache by ordered opt-ins', async () => {
    const calls: string[][] = [];
    const spawnPi = vi.fn((_executable: string, args: readonly string[]) => {
      const child = new EventEmitter() as ChildProcessWithoutNullStreams;
      const stdin = new PassThrough();
      const stdout = new PassThrough();
      const stderr = new PassThrough();
      Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
      stdin.on('finish', () => {
        calls.push([...args]);
        stdout.end(
          'provider  model  context  max-out  thinking  images\nfuture  model-v2  8K  1K  yes  no\n',
        );
        stderr.end();
        child.emit('close', 0, null);
      });
      return child;
    });
    const provider = new PiProvider({
      spawnPi,
      platform: 'linux',
      resolveCli: () => Promise.resolve({ ...identity, canonicalPath: '/opt/pi' }),
    });
    const signal = new AbortController().signal;
    const base = { providerId: 'pi' as const };
    await provider.listModels({ config: base, credential: null }, signal);
    await provider.listModels({ config: base, credential: null }, signal);
    await provider.listModels(
      {
        config: {
          ...base,
          piExtensionSources: [
            'tests/fixtures/pi-hung-tree.cjs',
            'tests/fixtures/validate-provider-logos.cjs',
          ],
        },
        credential: null,
      },
      signal,
    );
    await provider.listModels(
      {
        config: {
          ...base,
          piExtensionSources: [
            'tests/fixtures/validate-provider-logos.cjs',
            'tests/fixtures/pi-hung-tree.cjs',
          ],
        },
        credential: null,
      },
      signal,
    );

    expect(calls).toEqual([
      ['--list-models', ...PI_ISOLATION_FLAGS],
      [
        '--list-models',
        ...PI_ISOLATION_FLAGS,
        '-e',
        'tests/fixtures/pi-hung-tree.cjs',
        '-e',
        'tests/fixtures/validate-provider-logos.cjs',
      ],
      [
        '--list-models',
        ...PI_ISOLATION_FLAGS,
        '-e',
        'tests/fixtures/validate-provider-logos.cjs',
        '-e',
        'tests/fixtures/pi-hung-tree.cjs',
      ],
    ]);
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
    expect(calls[0]?.args[3]).not.toContain(' -e ');
    expect(calls[0]?.input).toBe('prompt & never shell-expanded');
    expect(calls[0]?.env.PI_CODING_AGENT_DIR).toBe('C:\\Users\\me\\.pi-custom');
  });
});

async function writePiCapabilityFixture(
  directory: string,
  name: string,
  repeatable: boolean,
): Promise<string> {
  const help = [
    '-p',
    '--list-models',
    '--model',
    '--thinking',
    ...PI_ISOLATION_FLAGS,
    `--extension, -e <path> Load extension${repeatable ? ' (can be used multiple times)' : ''}`,
  ].join(' ');
  const executable = resolve(directory, process.platform === 'win32' ? `${name}.cmd` : name);
  if (process.platform === 'win32') {
    await writeFile(
      executable,
      `@echo off\r\nif "%~1"=="--version" (\r\n  echo 1.0.0\r\n  exit /b 0\r\n)\r\nif "%~1"=="--help" (\r\n  echo ${help.replaceAll('<', '^<').replaceAll('>', '^>').replaceAll('(', '^(').replaceAll(')', '^)')}\r\n  exit /b 0\r\n)\r\nexit /b 1\r\n`,
      'utf8',
    );
  } else {
    await writeFile(
      executable,
      `#!/bin/sh\nif [ "$1" = "--version" ]; then echo 1.0.0; exit 0; fi\nif [ "$1" = "--help" ]; then echo '${help}'; exit 0; fi\nexit 1\n`,
      'utf8',
    );
    await chmod(executable, 0o755);
  }
  return executable;
}

function modelListSpawn(calls: string[][]) {
  return (_executable: string, args: readonly string[]) => {
    const child = new EventEmitter() as ChildProcessWithoutNullStreams;
    const stdin = new PassThrough();
    const stdout = new PassThrough();
    const stderr = new PassThrough();
    Object.assign(child, { stdin, stdout, stderr, pid: undefined, kill: () => true });
    stdin.on('finish', () => {
      calls.push([...args]);
      stdout.end(
        'provider  model  context  max-out  thinking  images\nfuture  model-v2  8K  1K  yes  no\n',
      );
      stderr.end();
      child.emit('close', 0, null);
    });
    return child;
  };
}

async function writeInstalledPiExtensionPackage(
  agentDirectory: string,
  packageName: string,
  version: string,
  manifestName = packageName,
): Promise<string> {
  const packageRoot = resolve(agentDirectory, 'npm', 'node_modules', ...packageName.split('/'));
  const extensionDirectory = resolve(packageRoot, 'extensions');
  await mkdir(extensionDirectory, { recursive: true });
  await writeFile(
    resolve(packageRoot, 'package.json'),
    JSON.stringify({
      name: manifestName,
      version,
      pi: { extensions: ['./extensions/index.ts'] },
    }),
    'utf8',
  );
  await writeFile(
    resolve(extensionDirectory, 'index.ts'),
    'export default function installedExtension() {}\n',
    'utf8',
  );
  return packageRoot;
}

function regularExtensionStats(): Stats {
  return {
    dev: 1,
    ino: 2,
    size: 3,
    mtimeMs: 4,
    isFile: () => true,
  } as Stats;
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}
