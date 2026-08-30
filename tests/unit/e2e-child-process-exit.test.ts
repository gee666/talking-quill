import { describe, expect, it, vi } from 'vitest';
import {
  waitForChildExit,
  type ChildProcessExitAdapter,
  type ChildProcessExitTimeouts,
} from '../e2e/child-process-exit';

const timeouts: ChildProcessExitTimeouts = { initialMs: 10, gracefulMs: 2, forcedMs: 2 };

function processAdapter(exitResults: readonly boolean[], forceFailure?: Error) {
  const remaining = [...exitResults];
  const waitForExit = vi.fn(() => Promise.resolve(remaining.shift() ?? false));
  const requestKill = vi.fn(() => true);
  const forceKillTree = vi.fn(() =>
    forceFailure === undefined ? Promise.resolve() : Promise.reject(forceFailure),
  );
  const adapter: ChildProcessExitAdapter = {
    pid: 1234,
    waitForExit,
    requestKill,
    forceKillTree,
  };
  return { adapter, forceKillTree, requestKill, waitForExit };
}

describe('source E2E child process teardown', () => {
  it('waits for confirmed exit after a graceful timeout kill', async () => {
    const process = processAdapter([false, true]);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'terminated after a normal kill request',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).not.toHaveBeenCalled();
    expect(process.waitForExit).toHaveBeenNthCalledWith(1, 10);
    expect(process.waitForExit).toHaveBeenNthCalledWith(2, 2);
  });

  it('escalates to forced tree termination and confirms exit', async () => {
    const process = processAdapter([false, false, true]);

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'required forced termination',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.waitForExit).toHaveBeenNthCalledWith(3, 2);
  });

  it('reports teardown failure when the process remains alive', async () => {
    const process = processAdapter([false, false, false], new Error('taskkill failed'));

    await expect(waitForChildExit(process.adapter, 'second instance', timeouts)).rejects.toThrow(
      'teardown failed after normal and forced termination attempts; pid=1234, normalKillRequested=true, forceError=Error: taskkill failed',
    );

    expect(process.requestKill).toHaveBeenCalledOnce();
    expect(process.forceKillTree).toHaveBeenCalledOnce();
    expect(process.waitForExit).toHaveBeenCalledTimes(3);
  });
});
