import type { MessagePortMain, WebContents } from 'electron';
import { describe, expect, it, vi } from 'vitest';
import { transferPort } from '../../app/src/main/ipc/port-transfer';

describe('MessagePort target-role transfer', () => {
  it('validates the declared role and descriptor before transfer', () => {
    const postMessage = vi.fn();
    const target = { postMessage } as unknown as WebContents;
    const port = {} as MessagePortMain;
    transferPort(target, 'capture', 'capture', 'capture:port', { protocolVersion: 3 }, port);
    expect(postMessage).toHaveBeenCalledWith('capture:port', { protocolVersion: 3 }, [port]);
  });

  it('rejects a mismatched role without transferring the port', () => {
    const postMessage = vi.fn();
    const target = { postMessage } as unknown as WebContents;
    expect(() =>
      transferPort(
        target,
        'capture',
        'main' as never,
        'capture:port',
        { protocolVersion: 3 },
        {} as MessagePortMain,
      ),
    ).toThrow('MessagePort target role is not allowed');
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('rejects an invalid descriptor before postMessage', () => {
    const postMessage = vi.fn();
    const target = { postMessage } as unknown as WebContents;
    expect(() =>
      transferPort(
        target,
        'capture',
        'capture',
        'capture:port',
        { protocolVersion: 2 } as never,
        {} as MessagePortMain,
      ),
    ).toThrow();
    expect(postMessage).not.toHaveBeenCalled();
  });
});
