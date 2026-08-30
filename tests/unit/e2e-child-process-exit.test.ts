import { type ChildProcess } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  createChildProcessExitAdapter,
  sourceE2EChildSpawnOptions,
  waitForChildExit,
  type ChildProcessExitAdapter,
  type ChildProcessExitTimeouts,
} from '../e2e/child-process-exit';

const timeouts: ChildProcessExitTimeouts = { initialMs: 10, gracefulMs: 2, forcedMs: 2 };

function childProcess(pid = 4321): ChildProcess {
  const child = new EventEmitter() as ChildProcess;
  Object.defineProperties(child, {
    pid: { value: pid },
    exitCode: { value: null, writable: true },
    signalCode: { value: null, writable: true },
  });
  child.kill = vi.fn(() => true);
  return child;
}

function processAdapter(
  exitResults: readonly boolean[],
  treeExitResults: readonly boolean[],
  forceFailure?: Error,
  forceTreeFirstOnTimeout = false,
) {
  const remaining = [...exitResults];
  const remainingTree = [...treeExitResults];
  const waitForExit = vi.fn(() => Promise.resolve(remaining.shift() ?? false));
  const waitForTreeExit = vi.fn(() => Promise.resolve(remainingTree.shift() ?? false));
  const requestKill = vi.fn(() => true);
  const forceKillTree = vi.fn(() =>
    forceFailure === undefined ? Promise.resolve() : Promise.reject(forceFailure),
  );
  const adapter: ChildProcessExitAdapter = {
    pid: 1234,
    forceTreeFirstOnTimeout,
    waitForExit,
    waitForTreeExit,
    requestKill,
    forceKillTree,
  };
  return { adapter, forceKillTree, requestKill, waitForExit, waitForTreeExit };
}

describe('source E2E child process teardown', () => {
  it('starts POSIX children in an owned process group without changing Windows spawning', () => {
    expect(sourceE2EChildSpawnOptions({}, 'linux')).toMatchObject({
      detached: true,
      stdio: 'ignore',
      windowsHide: true,
    });
    expect(sourceE2EChildSpawnOptions({}, 'win32')).not.toHaveProperty('detached');
  });

  it('force-terminates the owned POSIX group so descendants receive the signal', async () => {
    const child = childProcess();
    const killProcess = vi.fn();
    const adapter = createChildProcessExitAdapter(child, {
      platform: 'linux',
      ownsProcessGroup: true,
      killProcess,
      processGroupExists: () => true,
    });

    await adapter.forceKillTree();

    expect(killProcess).toHaveBeenCalledOnce();
    expect(killProcess).toHaveBeenCalledWith(-4321, 'SIGKILL');
  });

  it('does not signal an exited group whose PID may have been reused', async () => {
    const child = childProcess();
    const killProcess = vi.fn();
    const adapter = createChildProcessExitAdapter(child, {
      platform: 'linux',
      ownsProcessGroup: true,
      killProcess,
      processGroupExists: () => false,
    });
    child.emit('exit', 0, null);

    await adapter.forceKillTree();

    expect(killProcess).not.toHaveBeenCalled();
  });

  it('keeps Windows forced teardown on the exact taskkill tree path', async () => {
    const child = childProcess();
    const forceKillWindowsTree = vi.fn(() => Promise.resolve());
    const killProcess = vi.fn();
    const adapter = createChildProcessExitAdapter(child, {
      platform: 'win32',
      forceKillWindowsTree,
      killProcess,
    });

    await adapter.forceKillTree();

    expect(forceKillWindowsTree).toHaveBeenCalledOnce();
    expect(forceKillWindowsTree).toHaveBeenCalledWith(4321);
    expect(killProcess).not.toHaveBeenCalled();
  });

  it('runs Windows taskkill before a parent can exit and strand descendants', async () => {
    const process = processAdapter([false, true], [true], undefined, true);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'Windows process tree required forced termination',
    );

    expect(process.requestKill).not.toHaveBeenCalled();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.forceKillTree.mock.invocationCallOrder[0]).toBeLessThan(
      process.waitForExit.mock.invocationCallOrder[1] ?? Number.POSITIVE_INFINITY,
    );
  });

  it('waits for confirmed exit after a graceful timeout kill', async () => {
    const process = processAdapter([false, true], [true]);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'terminated after a normal kill request',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).not.toHaveBeenCalled();
    expect(process.waitForExit).toHaveBeenNthCalledWith(1, 10);
    expect(process.waitForExit).toHaveBeenNthCalledWith(2, 2);
  });

  it('force-cleans descendants when the parent exits after the normal kill', async () => {
    const process = processAdapter([false, true], [false, true]);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'owned process tree required forced termination',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.waitForExit).toHaveBeenCalledTimes(2);
    expect(process.waitForTreeExit).toHaveBeenCalledTimes(2);
  });

  it('escalates to forced tree termination and confirms exit', async () => {
    const process = processAdapter([false, false, true], [false, true]);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'required forced termination',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.waitForExit).toHaveBeenNthCalledWith(3, 2);
  });

  it('reports teardown failure when the process remains alive', async () => {
    const process = processAdapter(
      [false, false, false],
      [false, false],
      new Error('taskkill failed'),
    );

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'teardown failed after normal and forced termination attempts; pid=1234, normalKillRequested=true, parentExited=false, treeExited=false, forceError=Error: taskkill failed',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.waitForExit).toHaveBeenCalledTimes(3);
  });
});
