import { describe, expect, it, vi } from 'vitest';
import {
  InsertionService,
  clipboardTextSha256,
  type ClipboardAdapter,
} from '../../app/src/main/insertion/insertion-service';
import type { HelperActivationContext } from '../../app/src/shared/helper/protocol';

const ACTIVATION_CONTEXT: Readonly<HelperActivationContext> = Object.freeze({
  activationGeneration: 7,
  targetToken: 'opaque-target',
});

class FakeClipboard implements ClipboardAdapter {
  text = 'before';
  readonly formats = new Map<string, Uint8Array>([
    ['public.file-url', Uint8Array.from([1, 2, 3])],
    ['com.example.custom', Uint8Array.from([4, 5, 6])],
  ]);
  writeCount = 0;

  writeText(text: string): void {
    this.writeCount += 1;
    this.text = text;
    this.formats.clear();
    this.formats.set('public.utf8-plain-text', Buffer.from(text, 'utf8'));
  }

  externalChange(text: string): void {
    this.text = text;
    this.formats.clear();
    this.formats.set('public.file-url', Uint8Array.from([9, 8, 7]));
    this.formats.set('com.example.external', Uint8Array.from([6, 5, 4]));
  }
}

describe('InsertionService', () => {
  it('binds the exact canonical UTF-8 clipboard text with lowercase SHA-256', async () => {
    expect(clipboardTextSha256('')).toBe(
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    );
    expect(clipboardTextSha256('🦀\n')).toBe(
      '5d40fbf44301a6d80c06a5a5fb6aa8cdbb0c987d3aa07740e2bb941df3d7b862',
    );

    const clipboard = new FakeClipboard();
    const injectPaste = vi.fn(() => Promise.resolve({ submitted: true as const }));
    const delay = vi.fn(() => Promise.resolve());
    const service = new InsertionService(clipboard, { injectPaste }, delay);

    await expect(service.insert('🦀\n', ACTIVATION_CONTEXT)).resolves.toEqual({
      inserted: true,
      copied: false,
    });
    expect(injectPaste).toHaveBeenCalledWith(
      ACTIVATION_CONTEXT,
      clipboardTextSha256('🦀\n'),
      undefined,
      expect.any(Function),
    );
    expect(delay).toHaveBeenCalledWith(300);
    expect(clipboard.writeCount).toBe(1);
  });

  it('never snapshots or restores a subset of advertised clipboard formats', async () => {
    const clipboard = new FakeClipboard();
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true as const }) },
      () => {
        clipboard.externalChange('external clipboard');
        return Promise.resolve();
      },
    );

    await expect(service.insert('dictated text', ACTIVATION_CONTEXT)).resolves.toEqual({
      inserted: true,
      copied: false,
    });
    expect(clipboard.writeCount).toBe(1);
    expect(clipboard.text).toBe('external clipboard');
    expect([...clipboard.formats]).toEqual([
      ['public.file-url', Uint8Array.from([9, 8, 7])],
      ['com.example.external', Uint8Array.from([6, 5, 4])],
    ]);
  });

  it('cannot perform a torn clipboard read because the adapter is write-only', async () => {
    const clipboard = new FakeClipboard();
    const injectPaste = vi.fn(() =>
      Promise.resolve({ submitted: false as const, reason: 'unavailable' as const }),
    );
    const service = new InsertionService(clipboard, { injectPaste });

    await expect(service.insert('fallback', ACTIVATION_CONTEXT)).resolves.toEqual({
      inserted: false,
      copied: true,
    });
    expect(clipboard.text).toBe('fallback');
    expect(clipboard.writeCount).toBe(1);
  });

  it('keeps intended text for clipboard-only fallback without a target', async () => {
    const clipboard = new FakeClipboard();
    const injectPaste = vi.fn(() => Promise.resolve({ submitted: true as const }));
    const service = new InsertionService(clipboard, { injectPaste });

    await expect(
      service.insert('safe fallback', { activationGeneration: 8, targetToken: null }),
    ).resolves.toEqual({ inserted: false, copied: true });
    expect(injectPaste).not.toHaveBeenCalled();
    expect(clipboard.text).toBe('safe fallback');
  });

  it('keeps native commitment authoritative across a late response and cancellation', async () => {
    const clipboard = new FakeClipboard();
    const acknowledgement = deferred<
      { submitted: true } | { submitted: false; reason: 'unavailable' }
    >();
    const controller = new AbortController();
    const committed = vi.fn();
    const service = new InsertionService(
      clipboard,
      {
        injectPaste: (_context, _hash, _signal, onCommitted) => {
          onCommitted?.();
          return acknowledgement.promise;
        },
      },
      () => Promise.resolve(),
    );

    const insertion = service.insert(
      'committed text',
      ACTIVATION_CONTEXT,
      controller.signal,
      committed,
    );
    await vi.waitFor(() => expect(committed).toHaveBeenCalledOnce());
    controller.abort();
    acknowledgement.resolve({ submitted: false, reason: 'unavailable' });
    await expect(insertion).resolves.toEqual({ inserted: true, copied: false });
    expect(clipboard.text).toBe('committed text');
    expect(clipboard.writeCount).toBe(1);
  });

  it('preserves indeterminate native authority without claiming clipboard-only certainty', async () => {
    const clipboard = new FakeClipboard();
    const service = new InsertionService(clipboard, {
      injectPaste: () => Promise.resolve({ submitted: false, reason: 'indeterminate' }),
    });

    await expect(service.insert('uncertain text', ACTIVATION_CONTEXT)).resolves.toEqual({
      inserted: false,
      copied: true,
      indeterminate: true,
    });
    expect(clipboard.text).toBe('uncertain text');
  });

  it('leaves intended fallback text when cancellation wins before native dispatch', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const service = new InsertionService(clipboard, {
      injectPaste: (_context, _hash, signal) =>
        new Promise((_resolve, reject) => {
          signal?.addEventListener(
            'abort',
            () => reject(new DOMException('cancelled', 'AbortError')),
            { once: true },
          );
        }),
    });

    const insertion = service.insert('cancel fallback', ACTIVATION_CONTEXT, controller.signal);
    controller.abort();
    await expect(insertion).resolves.toEqual({
      inserted: false,
      copied: true,
      cancelled: true,
    });
    expect(clipboard.text).toBe('cancel fallback');
    expect(clipboard.writeCount).toBe(1);
  });

  it('does nothing when already aborted', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    controller.abort();
    const service = new InsertionService(clipboard, {
      injectPaste: () => Promise.resolve({ submitted: true as const }),
    });
    await expect(
      service.insert('never written', ACTIVATION_CONTEXT, controller.signal),
    ).resolves.toEqual({ inserted: false, copied: false, cancelled: true });
    expect(clipboard.text).toBe('before');
    expect(clipboard.writeCount).toBe(0);
  });
});

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
