import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { realpath, stat } from 'node:fs/promises';
import { win32 } from 'node:path';
import { ProviderError } from './errors';

const MAX_PROBE_BYTES = 512 * 1024;

export interface WindowsSystemTools {
  readonly systemRoot: string;
  readonly system32: string;
  readonly where: string;
  readonly cmd: string;
  readonly taskkill: string;
}

export type SpawnPi = (
  executable: string,
  args: readonly string[],
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

export function piSpawnCommand(
  executable: string,
  args: readonly string[],
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): { readonly executable: string; readonly args: readonly string[] } {
  if (platform !== 'win32' || !/\.(?:cmd|bat)$/iu.test(executable)) return { executable, args };
  if (!args.every(isSafePiArgument) || /["%!\r\n\0]/u.test(executable))
    throw new ProviderError('INVALID_CONFIG');
  const command = `""${executable}"${args.length === 0 ? '' : ` ${args.join(' ')}`}"`;
  return {
    executable: windowsSystemTools(environment).cmd,
    args: ['/d', '/s', '/c', command],
  };
}

function isSafePiArgument(value: string): boolean {
  return (
    /^--?[a-z][a-z-]*$/u.test(value) ||
    /^(?:off|minimal|low|medium|high|xhigh)$/u.test(value) ||
    /^[A-Za-z0-9][A-Za-z0-9._:@+/-]{0,511}$/u.test(value)
  );
}

export function windowsSystemTools(environment: NodeJS.ProcessEnv): WindowsSystemTools {
  assertNoCaseCollidingEnvironment(environment, 'win32');
  const source = environmentValue(environment, 'win32', 'SystemRoot');
  if (source === undefined || !/^[A-Za-z]:[\\/][^\0]*$/u.test(source))
    throw new ProviderError('PI_LAUNCH_FAILED');
  const systemRoot = win32.normalize(source);
  if (!/^[A-Za-z]:\\[^\0]*$/u.test(systemRoot) || systemRoot.includes('..'))
    throw new ProviderError('PI_LAUNCH_FAILED');
  const system32 = win32.join(systemRoot, 'System32');
  const expectedCmd = win32.join(system32, 'cmd.exe');
  const configuredComSpec = environmentValue(environment, 'win32', 'ComSpec');
  if (
    configuredComSpec !== undefined &&
    win32.normalize(configuredComSpec).toLowerCase() !== expectedCmd.toLowerCase()
  )
    throw new ProviderError('PI_LAUNCH_FAILED');
  return Object.freeze({
    systemRoot,
    system32,
    where: win32.join(system32, 'where.exe'),
    cmd: expectedCmd,
    taskkill: win32.join(system32, 'taskkill.exe'),
  });
}

export async function validateWindowsSystemTool(tool: string, system32: string): Promise<void> {
  const canonicalRoot = await realpath(system32);
  const canonicalTool = await realpath(tool);
  const metadata = await stat(canonicalTool);
  if (
    !metadata.isFile() ||
    win32.dirname(canonicalTool).toLowerCase() !== canonicalRoot.toLowerCase()
  )
    throw new ProviderError('PI_LAUNCH_FAILED');
}

export function assertNoCaseCollidingEnvironment(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): void {
  if (platform !== 'win32') return;
  const seen = new Set<string>();
  for (const [key, value] of Object.entries(source)) {
    if (value === undefined) continue;
    const normalized = key.toLowerCase();
    if (seen.has(normalized)) throw new ProviderError('PI_LAUNCH_FAILED');
    seen.add(normalized);
  }
}

export function environmentValue(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  key: string,
): string | undefined {
  if (platform !== 'win32') return source[key];
  return Object.entries(source).find(
    ([candidate, value]) => value !== undefined && candidate.toLowerCase() === key.toLowerCase(),
  )?.[1];
}

export async function runBoundedPiProcess(
  executable: string,
  args: readonly string[],
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  signal: AbortSignal | undefined,
  timeoutMs: number,
): Promise<{ readonly code: number; readonly stdout: string; readonly stderr: string }> {
  const command = piSpawnCommand(executable, args, environment, platform);
  let child: ChildProcessWithoutNullStreams;
  try {
    child = spawn(command.executable, command.args, {
      env: environment,
      shell: false,
      detached: platform !== 'win32',
      windowsHide: true,
      windowsVerbatimArguments: platform === 'win32' && /\.(?:cmd|bat)$/iu.test(executable),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
  } catch {
    throw new ProviderError('PI_LAUNCH_FAILED');
  }
  child.stdin.end();
  return await new Promise((resolveResult, rejectResult) => {
    let settled = false;
    let terminating = false;
    let bytes = 0;
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    const finish = (error: ProviderError | null, code = 1): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      if (error) rejectResult(error);
      else
        resolveResult({
          code,
          stdout: Buffer.concat(stdout).toString('utf8'),
          stderr: Buffer.concat(stderr).toString('utf8'),
        });
    };
    const terminate = (error: ProviderError): void => {
      if (settled || terminating) return;
      terminating = true;
      void terminateProbeProcess(child, platform, environment).then(
        () => finish(error),
        () => finish(new ProviderError('PI_LAUNCH_FAILED')),
      );
    };
    const abort = (): void => terminate(new ProviderError('CANCELLED'));
    const timer = setTimeout(() => terminate(new ProviderError('TIMEOUT')), timeoutMs);
    timer.unref();
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted === true) abort();
    const collect = (target: Buffer[]) => (chunk: Buffer) => {
      bytes += chunk.byteLength;
      if (bytes > MAX_PROBE_BYTES) terminate(new ProviderError('RESPONSE_TOO_LARGE'));
      else target.push(Buffer.from(chunk));
    };
    child.stdout.on('data', collect(stdout));
    child.stderr.on('data', collect(stderr));
    child.once('error', () => terminate(new ProviderError('PI_LAUNCH_FAILED')));
    child.once('close', (code) => {
      if (!terminating) finish(null, code ?? 1);
    });
  });
}

async function terminateProbeProcess(
  child: ChildProcessWithoutNullStreams,
  platform: NodeJS.Platform,
  environment: NodeJS.ProcessEnv,
): Promise<void> {
  const pid = child.pid;
  if (pid === undefined) {
    child.kill('SIGKILL');
    return;
  }
  if (platform !== 'win32') {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {
      child.kill('SIGKILL');
    }
    const deadline = Date.now() + 2_000;
    while (processGroupExists(pid) && Date.now() < deadline)
      await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
    if (processGroupExists(pid)) throw new Error('Pi probe process group did not terminate');
    return;
  }
  const tools = windowsSystemTools(environment);
  await validateWindowsSystemTool(tools.taskkill, tools.system32);
  let killer: ChildProcessWithoutNullStreams;
  try {
    killer = spawn(tools.taskkill, ['/pid', String(pid), '/t', '/f'], {
      shell: false,
      windowsHide: true,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    killer.stdin.end();
  } catch {
    child.kill('SIGKILL');
    throw new Error('Pi probe tree killer did not start');
  }
  const succeeded = await new Promise<boolean>((resolveKill) => {
    let done = false;
    const finish = (value: boolean): void => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolveKill(value);
    };
    const timer = setTimeout(() => {
      killer.kill('SIGKILL');
      finish(false);
    }, 2_000);
    timer.unref();
    killer.once('error', () => finish(false));
    killer.once('close', (code) => finish(code === 0));
  });
  if (!succeeded) {
    child.kill('SIGKILL');
    throw new Error('Pi probe tree killer failed');
  }
  const deadline = Date.now() + 2_000;
  while (processExists(pid) && Date.now() < deadline)
    await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
  if (processExists(pid)) throw new Error('Pi probe root did not terminate');
}

export async function runPiInvocation(
  executable: string,
  args: readonly string[],
  input: string | null,
  signal: AbortSignal,
  timeoutMs: number,
  options: {
    readonly spawnPi: SpawnPi | undefined;
    readonly environment: NodeJS.ProcessEnv;
    readonly platform: NodeJS.Platform;
    readonly workingDirectory: string;
    readonly maxStdoutBytes: number;
    readonly maxStderrBytes: number;
  },
): Promise<{ readonly code: number; readonly stdout: string; readonly stderr: string }> {
  const command = piSpawnCommand(executable, args, options.environment, options.platform);
  return await new Promise((resolveOutput, rejectOutput) => {
    let settled = false;
    let terminating = false;
    let stdoutBytes = 0;
    let stderrBytes = 0;
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let child: ChildProcessWithoutNullStreams;
    const finish = (
      error: ProviderError | null,
      result: { readonly code: number; readonly stdout: string; readonly stderr: string } = {
        code: 1,
        stdout: '',
        stderr: '',
      },
    ): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      if (error === null) resolveOutput(result);
      else rejectOutput(error);
    };
    const terminate = (error: ProviderError): void => {
      if (settled || terminating) return;
      terminating = true;
      void terminateProcessTree(child, options.platform, options.environment).then(
        () => finish(error),
        () => finish(new ProviderError('PI_LAUNCH_FAILED')),
      );
    };
    const abort = (): void => terminate(new ProviderError('CANCELLED'));
    try {
      child = (options.spawnPi ?? spawn)(command.executable, command.args, {
        env: options.environment,
        cwd: options.workingDirectory,
        shell: false,
        windowsHide: true,
        windowsVerbatimArguments:
          options.platform === 'win32' && /\.(?:cmd|bat)$/iu.test(executable),
        detached: options.platform !== 'win32',
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    } catch {
      rejectOutput(new ProviderError('PI_LAUNCH_FAILED'));
      return;
    }
    const timer = setTimeout(() => terminate(new ProviderError('TIMEOUT')), timeoutMs);
    timer.unref();
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) {
      abort();
      return;
    }
    child.stdout.on('data', (chunk: Buffer) => {
      stdoutBytes += chunk.byteLength;
      if (stdoutBytes > options.maxStdoutBytes) terminate(new ProviderError('RESPONSE_TOO_LARGE'));
      else stdout.push(Buffer.from(chunk));
    });
    child.stderr.on('data', (chunk: Buffer) => {
      stderrBytes += chunk.byteLength;
      if (stderrBytes > options.maxStderrBytes) terminate(new ProviderError('RESPONSE_TOO_LARGE'));
      else stderr.push(Buffer.from(chunk));
    });
    child.once('error', () => terminate(new ProviderError('PI_LAUNCH_FAILED')));
    child.once('close', (code) => {
      if (settled || terminating) return;
      finish(null, {
        code: code ?? 1,
        stdout: Buffer.concat(stdout, stdoutBytes).toString('utf8'),
        stderr: Buffer.concat(stderr, stderrBytes).toString('utf8'),
      });
    });
    child.stdin.on('error', () => terminate(new ProviderError('PI_LAUNCH_FAILED')));
    if (input === null) child.stdin.end();
    else child.stdin.end(input, 'utf8');
  });
}

interface PiTreeKiller {
  once(event: 'error', listener: () => void): unknown;
  once(event: 'close', listener: (code: number | null) => void): unknown;
  kill(): boolean;
}

export interface PiTreeTerminationOptions {
  readonly spawnKiller?: (executable: string, args: readonly string[]) => PiTreeKiller;
  readonly processExists?: (pid: number) => boolean;
  readonly listDescendants?: (pid: number) => Promise<readonly number[]>;
}

export async function terminateProcessTree(
  child: ChildProcessWithoutNullStreams,
  platform: NodeJS.Platform,
  environment: NodeJS.ProcessEnv,
  options: PiTreeTerminationOptions = {},
): Promise<void> {
  const pid = child.pid;
  if (pid === undefined) {
    child.kill('SIGKILL');
    return;
  }
  if (platform === 'win32') {
    const tools = windowsSystemTools(environment);
    let killer: PiTreeKiller;
    try {
      killer =
        options.spawnKiller?.(tools.taskkill, ['/pid', String(pid), '/t', '/f']) ??
        spawn(tools.taskkill, ['/pid', String(pid), '/t', '/f'], {
          env: environment,
          shell: false,
          windowsHide: true,
          stdio: 'ignore',
        });
    } catch {
      child.kill('SIGKILL');
      throw new Error('Pi process-tree killer did not start');
    }
    const taskkillSucceeded = await new Promise<boolean>((resolveDone) => {
      let settled = false;
      const finish = (succeeded: boolean): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolveDone(succeeded);
      };
      const timer = setTimeout(() => {
        killer.kill();
        finish(false);
      }, 2_000);
      timer.unref();
      killer.once('error', () => finish(false));
      killer.once('close', (code) => finish(code === 0));
    });
    if (!taskkillSucceeded) {
      child.kill('SIGKILL');
      throw new Error('Pi process-tree killer failed');
    }
    const exists = options.processExists ?? processExists;
    const deadline = Date.now() + 2_000;
    while (exists(pid) && Date.now() < deadline)
      await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
    if (exists(pid)) throw new Error('Pi process tree did not terminate');
    return;
  }
  try {
    process.kill(-pid, 'SIGKILL');
  } catch {
    child.kill('SIGKILL');
  }
  const deadline = Date.now() + 2_000;
  while (processGroupExists(pid) && Date.now() < deadline)
    await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
  if (processGroupExists(pid)) throw new Error('Pi process group did not terminate');
}

function processGroupExists(pid: number): boolean {
  try {
    process.kill(-pid, 0);
    return true;
  } catch {
    return false;
  }
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}
