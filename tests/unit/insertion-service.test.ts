import { describe, expect, it, vi } from 'vitest';
import {
  InsertionService,
  clipboardFingerprint,
  type ClipboardAdapter,
  type ClipboardSnapshot,
} from '../../app/src/main/insertion/insertion-service';

class FakeClipboard implements ClipboardAdapter {
  value = 'before';
  html = '<b>before</b>';
  rtf = '{\\rtf1 before}';
  imagePng: Uint8Array | null = Uint8Array.from([1, 2, 3]);
  restored: ClipboardSnapshot | null = null;
  readonly original: ClipboardSnapshot = {
    text: 'before',
    html: '<b>before</b>',
    rtf: '{\\rtf1 before}',
    imagePng: Uint8Array.from([1, 2, 3]),
  };
  snapshot() {
    return { text: this.value, html: this.html, rtf: this.rtf, imagePng: this.imagePng };
  }
  writeText(text: string) {
    this.value = text;
    this.html = '';
    this.rtf = '';
    this.imagePng = null;
  }
  restore(snapshot: ClipboardSnapshot) {
    this.restored = snapshot;
    this.value = snapshot.text;
    this.html = snapshot.html;
    this.rtf = snapshot.rtf;
    this.imagePng = snapshot.imagePng;
  }
}

describe('InsertionService', () => {
  it('uses typed length framing with no delimiter or image-presence ambiguity', () => {
    const delimiterLeft = clipboardFingerprint({
      text: 'a\0b',
      html: 'c',
      rtf: '',
      imagePng: null,
    });
    const delimiterRight = clipboardFingerprint({
      text: 'a',
      html: 'b\0c',
      rtf: '',
      imagePng: null,
    });
    const absentImage = clipboardFingerprint({ text: 'same', html: '', rtf: '', imagePng: null });
    const emptyImage = clipboardFingerprint({
      text: 'same',
      html: '',
      rtf: '',
      imagePng: new Uint8Array(),
    });
    expect(delimiterLeft).not.toBe(delimiterRight);
    expect(absentImage).not.toBe(emptyImage);
  });

  it('swaps, pastes, waits 300 ms, and restores text/HTML/RTF/image', async () => {
    const clipboard = new FakeClipboard();
    const delay = vi.fn(() => Promise.resolve());
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      delay,
    );
    await expect(service.insert('dictated text')).resolves.toEqual({
      inserted: true,
      copied: false,
    });
    expect(delay).toHaveBeenCalledWith(300);
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it('marks insertion committed before restoration and ignores late cancellation', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const restore = deferred<undefined>();
    const committed = vi.fn(() => controller.abort());
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      () => restore.promise,
    );

    const insertion = service.insert('dictated text', controller.signal, committed);
    await vi.waitFor(() => expect(committed).toHaveBeenCalledOnce());
    expect(clipboard.restored).toBeNull();
    restore.resolve(undefined);
    await expect(insertion).resolves.toEqual({ inserted: true, copied: false });
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it('restores and cancels when cancellation wins before failed paste dispatch', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const paste = deferred<{ submitted: boolean }>();
    const committed = vi.fn();
    const service = new InsertionService(clipboard, { injectPaste: () => paste.promise });
    const insertion = service.insert('dictated text', controller.signal, committed);

    controller.abort();
    paste.resolve({ submitted: false });
    await expect(insertion).resolves.toEqual({
      inserted: false,
      copied: false,
      cancelled: true,
    });
    expect(committed).not.toHaveBeenCalled();
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it('restores the original clipboard when shutdown aborts during the post-paste delay', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const delayControl: { release: (() => void) | null } = { release: null };
    const delay = new Promise<void>((resolve) => {
      delayControl.release = resolve;
    });
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      () => delay,
    );
    const insertion = service.insert('dictated text', controller.signal);
    await vi.waitFor(() => expect(clipboard.value).toBe('dictated text'));
    controller.abort();
    if (delayControl.release === null) throw new Error('Delay was not armed');
    delayControl.release();

    await expect(insertion).resolves.toEqual({ inserted: true, copied: false });
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it('does not overwrite a clipboard changed by the user during the restore delay', async () => {
    const clipboard = new FakeClipboard();
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      () => {
        clipboard.value = 'new user clipboard';
        return Promise.resolve();
      },
    );
    await service.insert('dictated text');
    expect(clipboard.value).toBe('new user clipboard');
    expect(clipboard.restored).toBeNull();
  });

  it.each([
    {
      name: 'new rich HTML with the same text',
      change: (clipboard: FakeClipboard) => {
        clipboard.html = '<b>dictated text</b>';
      },
    },
    {
      name: 'new rich RTF with the same text',
      change: (clipboard: FakeClipboard) => {
        clipboard.rtf = '{\\rtf1 dictated text}';
      },
    },
    {
      name: 'new image with the same text',
      change: (clipboard: FakeClipboard) => {
        clipboard.imagePng = Uint8Array.from([9, 8, 7]);
      },
    },
  ])('does not restore over $name', async ({ change }) => {
    const clipboard = new FakeClipboard();
    const service = new InsertionService(
      clipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      () => {
        change(clipboard);
        return Promise.resolve();
      },
    );
    await service.insert('dictated text');
    expect(clipboard.value).toBe('dictated text');
    expect(clipboard.restored).toBeNull();
  });

  it.each([
    { injectPaste: () => Promise.resolve({ submitted: false }) },
    { injectPaste: () => Promise.reject(new Error('helper unavailable')) },
  ])('leaves text copied when native injection fails', async (paste) => {
    const clipboard = new FakeClipboard();
    const service = new InsertionService(clipboard, paste);
    await expect(service.insert('fallback text')).resolves.toEqual({
      inserted: false,
      copied: true,
    });
    expect(clipboard.value).toBe('fallback text');
    expect(clipboard.restored).toBeNull();
  });

  it('cancels and restores when abort wins before native dispatch', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const committed = vi.fn();
    const service = new InsertionService(clipboard, {
      injectPaste: (signal, onCommitted) =>
        new Promise((_resolve, reject) => {
          signal?.addEventListener(
            'abort',
            () => {
              expect(onCommitted).toBeDefined();
              reject(new DOMException('cancelled before dispatch', 'AbortError'));
            },
            { once: true },
          );
        }),
    });
    const insertion = service.insert('temporary', controller.signal, committed);
    await vi.waitFor(() => expect(clipboard.value).toBe('temporary'));
    controller.abort();
    await expect(insertion).resolves.toEqual({
      inserted: false,
      copied: false,
      cancelled: true,
    });
    expect(committed).not.toHaveBeenCalled();
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it.each([
    { name: 'submitted true', settle: 'true' as const },
    { name: 'submitted false', settle: 'false' as const },
    { name: 'transport rejection', settle: 'reject' as const },
  ])('keeps native commit authoritative across late $name acknowledgement', async ({ settle }) => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    const acknowledgement = deferred<{ submitted: boolean }>();
    const committed = vi.fn();
    const service = new InsertionService(
      clipboard,
      {
        injectPaste: (_signal, onCommitted) => {
          onCommitted?.();
          return acknowledgement.promise;
        },
      },
      () => Promise.resolve(),
    );
    const insertion = service.insert('committed text', controller.signal, committed);
    await vi.waitFor(() => expect(committed).toHaveBeenCalledOnce());
    controller.abort();
    if (settle === 'true') acknowledgement.resolve({ submitted: true });
    else if (settle === 'false') acknowledgement.resolve({ submitted: false });
    else acknowledgement.reject(new Error('late acknowledgement failure'));
    await expect(insertion).resolves.toEqual({ inserted: true, copied: false });
    expect(committed).toHaveBeenCalledOnce();
    expect(clipboard.restored).toEqual(clipboard.original);
  });

  it('bounds never-resolving acknowledgement on both sides of the commit boundary', async () => {
    vi.useFakeTimers();
    const uncommittedClipboard = new FakeClipboard();
    const controller = new AbortController();
    const uncommitted = new InsertionService(uncommittedClipboard, {
      injectPaste: () => new Promise(() => undefined),
    }).insert('temporary', controller.signal);
    controller.abort();
    await vi.advanceTimersByTimeAsync(3_500);
    await expect(uncommitted).resolves.toEqual({
      inserted: false,
      copied: false,
      cancelled: true,
    });
    expect(uncommittedClipboard.restored).toEqual(uncommittedClipboard.original);

    const committedClipboard = new FakeClipboard();
    const committed = vi.fn();
    const committedInsertion = new InsertionService(
      committedClipboard,
      {
        injectPaste: (_signal, onCommitted) => {
          onCommitted?.();
          return new Promise(() => undefined);
        },
      },
      () => Promise.resolve(),
    ).insert('inserted', undefined, committed);
    await vi.advanceTimersByTimeAsync(3_500);
    await expect(committedInsertion).resolves.toEqual({ inserted: true, copied: false });
    expect(committed).toHaveBeenCalledOnce();
    expect(committedClipboard.restored).toEqual(committedClipboard.original);
  });

  it('bounds a hanging helper as copied and a hanging restore as committed', async () => {
    vi.useFakeTimers();
    const helperClipboard = new FakeClipboard();
    const helperService = new InsertionService(helperClipboard, {
      injectPaste: () => new Promise(() => undefined),
    });
    const helperInsertion = helperService.insert('copied');
    await vi.advanceTimersByTimeAsync(3_500);
    await expect(helperInsertion).resolves.toEqual({ inserted: false, copied: true });

    const restoreClipboard = new FakeClipboard();
    const committed = vi.fn();
    const restoreService = new InsertionService(
      restoreClipboard,
      { injectPaste: () => Promise.resolve({ submitted: true }) },
      () => new Promise(() => undefined),
    );
    const restoreInsertion = restoreService.insert('inserted', undefined, committed);
    await vi.advanceTimersByTimeAsync(1);
    expect(committed).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1_000);
    await expect(restoreInsertion).resolves.toEqual({ inserted: true, copied: false });
    expect(restoreClipboard.restored).toEqual(restoreClipboard.original);
  });

  it('does nothing when already aborted', async () => {
    const clipboard = new FakeClipboard();
    const controller = new AbortController();
    controller.abort();
    const service = new InsertionService(clipboard, {
      injectPaste: () => Promise.resolve({ submitted: true }),
    });
    await expect(service.insert('never written', controller.signal)).resolves.toEqual({
      inserted: false,
      copied: false,
      cancelled: true,
    });
    expect(clipboard.value).toBe('before');
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
