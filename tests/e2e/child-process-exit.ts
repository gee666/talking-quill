import { execFile, type ChildProcess } from 'node:child_process';
import { resolve } from 'node:path';

export interface ChildProcessExitAdapter {
  readonly pid: number | undefined;
  waitForExit(timeoutMs: number): Promise<boolean>;
  requestKill(): boolean;
  forceKillTree(): Promise<void>;
}

export interface ChildProcessExitTimeouts {
  readonly initialMs: number;
  readonly gracefulMs: number;
  readonly forcedMs: number;
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
  if (await adapter.waitForExit(timeouts.gracefulMs)) {
    throw new Error(
      `${label} did not exit within ${String(timeouts.initialMs)} ms; terminated after a normal kill request`,
    );
  }

  let forceFailure: unknown;
  try {
    await adapter.forceKillTree();
  } catch (error: unknown) {
    forceFailure = error;
  }
  if (await adapter.waitForExit(timeouts.forcedMs)) {
    throw new Error(
      `${label} did not exit within ${String(timeouts.initialMs)} ms; required forced termination`,
    );
  }

  const details = [
    `pid=${String(adapter.pid ?? 'unavailable')}`,
    `normalKillRequested=${String(killRequested)}`,
    ...(normalKillFailure === undefined
      ? []
      : [`normalKillError=${formatError(normalKillFailure)}`]),
    ...(forceFailure === undefined ? [] : [`forceError=${formatError(forceFailure)}`]),
  ].join(', ');
  throw new Error(
    `${label} teardown failed after normal and forced termination attempts; ${details}`,
  );
}

export function createChildProcessExitAdapter(child: ChildProcess): ChildProcessExitAdapter {
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
    requestKill: () => child.kill(),
    forceKillTree: () => forceKillTree(child),
  };
}

async function forceKillTree(child: ChildProcess): Promise<void> {
  if (process.platform !== 'win32') {
    if (!child.kill('SIGKILL')) throw new Error('SIGKILL request was rejected');
    return;
  }
  const pid = child.pid;
  if (pid === undefined) throw new Error('Child process ID is unavailable');
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
