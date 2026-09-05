import { createHash } from 'node:crypto';
import { clipboard } from 'electron';
import { ECHO_CLIPBOARD_RESTORE_MS as ECHO_POST_COMMIT_SETTLE_MS } from '../../shared/constants/echo-session';
import type { HelperActivationContext, HelperPasteResult } from '../../shared/helper/protocol';
import type { HelperClient } from '../helper';

export interface ClipboardAdapter {
  writeText(text: string): void;
}

export interface PasteAdapter {
  injectPaste(
    activationContext: Readonly<HelperActivationContext>,
    expectedClipboardSha256: string,
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<HelperPasteResult>;
}

export const INSERTION_DISPATCH_TIMEOUT_MS = 3_500;
export const INSERTION_COMMIT_SETTLE_TIMEOUT_MS = 1_000;

export interface InsertionResult {
  readonly inserted: boolean;
  readonly copied: boolean;
  readonly cancelled?: boolean;
  /** Native injection was claimed but completion was not proved; never retry. */
  readonly indeterminate?: boolean;
}

export class InsertionService {
  readonly #clipboard: ClipboardAdapter;
  readonly #paste: PasteAdapter;
  readonly #delay: (milliseconds: number) => Promise<void>;

  constructor(
    clipboardAdapter: ClipboardAdapter,
    paste: PasteAdapter,
    delay: (milliseconds: number) => Promise<void> = defaultDelay,
  ) {
    this.#clipboard = clipboardAdapter;
    this.#paste = paste;
    this.#delay = delay;
  }

  async insert(
    text: string,
    activationContext: Readonly<HelperActivationContext>,
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<InsertionResult> {
    if (isAborted(signal)) return { inserted: false, copied: false, cancelled: true };

    // This is the sole clipboard mutation. The intended plain text remains the
    // authoritative clipboard-only fallback; neither success, cancellation nor
    // delayed completion snapshots/restores a subset of pasteboard formats.
    this.#clipboard.writeText(text);
    const expectedClipboardSha256 = clipboardTextSha256(text);
    if (activationContext.targetToken === null) {
      console.error('dictation paste fallback: activation target unavailable');
      return { inserted: false, copied: true };
    }

    let submitted = false;
    let indeterminate = false;
    let nativeCommitted = false;
    let commitPublished = false;
    const publishCommit = (): void => {
      nativeCommitted = true;
      if (commitPublished) return;
      commitPublished = true;
      onCommitted?.();
    };
    try {
      const pasteResult = await boundedOperation(
        this.#paste.injectPaste(activationContext, expectedClipboardSha256, signal, publishCommit),
        INSERTION_DISPATCH_TIMEOUT_MS,
      );
      submitted = pasteResult.submitted;
      if (!pasteResult.submitted) console.error('dictation paste fallback:', pasteResult.reason);
      indeterminate = !pasteResult.submitted && pasteResult.reason === 'indeterminate';
    } catch {
      console.error('dictation paste fallback: native request failed or timed out');
      submitted = false;
    }
    submitted ||= nativeCommitted;
    if (!submitted) {
      if (indeterminate) return { inserted: false, copied: true, indeterminate: true };
      if (isAborted(signal)) return { inserted: false, copied: true, cancelled: true };
      return { inserted: false, copied: true };
    }

    publishCommit();
    await boundedOperation(
      this.#delay(ECHO_POST_COMMIT_SETTLE_MS),
      INSERTION_COMMIT_SETTLE_TIMEOUT_MS,
    ).catch(() => undefined);
    return { inserted: true, copied: false };
  }
}

export class ElectronClipboardAdapter implements ClipboardAdapter {
  writeText(text: string): void {
    clipboard.writeText(text);
  }
}

/** SHA-256 of the exact UTF-8 bytes written by Electron, encoded as lowercase hex. */
export function clipboardTextSha256(text: string): string {
  return createHash('sha256').update(Buffer.from(text, 'utf8')).digest('hex');
}

export function createInsertionService(helper: HelperClient): InsertionService {
  return new InsertionService(new ElectronClipboardAdapter(), helper);
}

function defaultDelay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function isAborted(signal?: AbortSignal): boolean {
  return signal?.aborted ?? false;
}

function boundedOperation<Value>(
  operation: Promise<Value>,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<Value> {
  if (signal?.aborted === true) return Promise.reject(abortError());
  return new Promise<Value>((resolve, reject) => {
    const finish = (callback: () => void): void => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      callback();
    };
    const timer = setTimeout(
      () => finish(() => reject(new Error('Insertion operation timed out'))),
      timeoutMs,
    );
    timer.unref();
    const abort = (): void => finish(() => reject(abortError()));
    signal?.addEventListener('abort', abort, { once: true });
    operation.then(
      (value) => finish(() => resolve(value)),
      (error: unknown) =>
        finish(() =>
          reject(error instanceof Error ? error : new Error('Insertion operation failed')),
        ),
    );
  });
}

function abortError(): DOMException {
  return new DOMException('Insertion operation cancelled', 'AbortError');
}
