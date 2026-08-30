import { execFile, type ChildProcess, type SpawnOptions } from 'node:child_process';
import { resolve } from 'node:path';

export interface ChildProcessExitAdapter {
  readonly pid: number | undefined;
  waitForExit(timeoutMs: number): Promise<boolean>;
  waitForTreeExit(timeoutMs: number): Promise<boolean>;
  requestKill(): boolean;
  forceKillTree(): Promise<void>;
}

export interface ChildProcessExitTimeouts {
  readonly initialMs: number;
  readonly gracefulMs: number;
  readonly forcedMs: number;
}

export interface ChildProcessExitAdapterOptions {
  readonly platform?: NodeJS.Platform;
  readonly ownsProcessGroup?: boolean;
  readonly killProcess?: (pid: number, signal: 'SIGKILL') => void;
  readonly processGroupExists?: (processGroupId: number) => boolean;
  readonly forceKillWindowsTree?: (pid: number) => Promise<void>;
}

export function sourceE2EChildSpawnOptions(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform = process.platform,
): SpawnOptions {
  return {
    stdio: 'ignore',
    windowsHide: true,
    env: environment,
    ...(platform === 'win32' ? {} : { detached: true }),
  };
}

const defaultTimeouts: ChildProcessExitTimeouts = {
  initialMs: 10_000,
  gracefulMs: 2_000,
  forcedMs: 2_000,
};

export async function waitForChildExit(
  adapter: ChildProcessExitAdapter,
  label: string,
  timeouts: ChildProcessExitTimeouts = defaultTimeouts,
): Promise<void> {
  if (await adapter.waitForExit(timeouts.initialMs)) return;

  let killRequested = false;
  let normalKillFailure: unknown;
  try {
    killRequested = adapter.requestKill();
  } catch (error: unknown) {
    normalKillFailure = error;
  }
  const [parentExitedAfterNormalKill, treeExitedAfterNormalKill] = await Promise.all([
    adapter.waitForExit(timeouts.gracefulMs),
    adapter.waitForTreeExit(timeouts.gracefulMs),
  ]);
  if (treeExitedAfterNormalKill) {
    throw new Error(
      `${label} did not exit within ${String(timeouts.initialMs)} ms; owned process tree terminated after a normal kill request`,
    );
  }

  let forceFailure: unknown;
  try {
    await adapter.forceKillTree();
  } catch (error: unknown) {
    forceFailure = error;
  }
  const [parentExitedAfterForce, treeExitedAfterForce] = await Promise.all([
    parentExitedAfterNormalKill ? Promise.resolve(true) : adapter.waitForExit(timeouts.forcedMs),
    adapter.waitForTreeExit(timeouts.forcedMs),
  ]);
  if (treeExitedAfterForce && parentExitedAfterForce) {
    throw new Error(
      `${label} did not exit within ${String(timeouts.initialMs)} ms; owned process tree required forced termination`,
    );
  }

  const details = [
    `pid=${String(adapter.pid ?? 'unavailable')}`,
    `normalKillRequested=${String(killRequested)}`,
    ...(normalKillFailure === undefined
      ? []
      : [`normalKillError=${formatError(normalKillFailure)}`]),
    `parentExited=${String(parentExitedAfterForce)}`,
    `treeExited=${String(treeExitedAfterForce)}`,
    ...(forceFailure === undefined ? [] : [`forceError=${formatError(forceFailure)}`]),
  ].join(', ');
  throw new Error(
    `${label} teardown failed after normal and forced termination attempts; ${details}`,
  );
}

export function createChildProcessExitAdapter(
  child: ChildProcess,
  options: ChildProcessExitAdapterOptions = {},
): ChildProcessExitAdapter {
  let resolveExit: (() => void) | undefined;
  const exitPromise = new Promise<void>((resolveWait) => {
    resolveExit = resolveWait;
  });
  const onExit = () => {
    child.removeListener('error', onError);
    resolveExit?.();
  };
  // A failed kill can emit "error" without an exit. Keep observing the process and escalate.
  const onError = () => undefined;
  if (child.exitCode === null && child.signalCode === null) {
    child.once('exit', onExit);
    child.on('error', onError);
  } else {
    resolveExit?.();
  }

  const platform = options.platform ?? process.platform;
  const ownsProcessGroup = platform !== 'win32' && options.ownsProcessGroup === true;
  let treeGone = false;
  const processGroupExists = options.processGroupExists ?? defaultProcessGroupExists;

  return {
    pid: child.pid,
    async waitForExit(timeoutMs) {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const exited = await Promise.race([
        exitPromise.then(() => true),
        new Promise<false>((resolveTimeout) => {
          timer = setTimeout(() => resolveTimeout(false), timeoutMs);
        }),
      ]);
      if (timer !== undefined) clearTimeout(timer);
      return exited;
    },
    async waitForTreeExit(timeoutMs) {
      if (!ownsProcessGroup) return this.waitForExit(timeoutMs);
      const pid = child.pid;
      if (pid === undefined) throw new Error('Child process ID is unavailable');
      treeGone ||= await waitForAbsence(() => processGroupExists(pid), timeoutMs);
      return treeGone;
    },
    requestKill: () => child.kill(),
    forceKillTree: () =>
      forceKillTree(
        child,
        () => treeGone,
        () => {
          treeGone = true;
        },
        options,
      ),
  };
}

async function forceKillTree(
  child: ChildProcess,
  treeHasGone: () => boolean,
  markTreeGone: () => void,
  options: ChildProcessExitAdapterOptions,
): Promise<void> {
  if (treeHasGone()) return;
  const pid = child.pid;
  if (pid === undefined) throw new Error('Child process ID is unavailable');
  const platform = options.platform ?? process.platform;
  if (platform === 'win32') {
    if (child.exitCode !== null || child.signalCode !== null) return;
    await (options.forceKillWindowsTree ?? forceKillWindowsTree)(pid);
    return;
  }
  if (options.ownsProcessGroup !== true || pid <= 1 || pid === process.pid) {
    throw new Error('Refusing to signal a process group not owned by this E2E child');
  }
  const processGroupExists = options.processGroupExists ?? defaultProcessGroupExists;
  if (!processGroupExists(pid)) {
    markTreeGone();
    return;
  }
  try {
    (options.killProcess ?? process.kill)(-pid, 'SIGKILL');
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
  }
}

function defaultProcessGroupExists(processGroupId: number): boolean {
  try {
    process.kill(-processGroupId, 0);
    return true;
  } catch (error: unknown) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ESRCH') return false;
    if (code === 'EPERM') return true;
    throw error;
  }
}

async function waitForAbsence(exists: () => boolean, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (exists()) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) return false;
    await new Promise<void>((resolveWait) => {
      setTimeout(resolveWait, Math.min(25, remaining));
    });
  }
  return true;
}

async function forceKillWindowsTree(pid: number): Promise<void> {
  const taskkill = resolve(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'taskkill.exe');
  await new Promise<void>((resolveKill, reject) => {
    execFile(
      taskkill,
      ['/PID', String(pid), '/T', '/F'],
      { windowsHide: true, timeout: 2_000, killSignal: 'SIGKILL' },
      (error) => {
        if (error === null) resolveKill();
        else reject(new Error('Forced process-tree termination failed', { cause: error }));
      },
    );
  });
}

function formatError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  if (typeof error === 'string') return error;
  return 'unknown error';
}
