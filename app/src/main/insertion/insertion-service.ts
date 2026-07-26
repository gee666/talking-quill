import { createHash } from 'node:crypto';
import { clipboard, nativeImage } from 'electron';
import { ECHO_CLIPBOARD_RESTORE_MS } from '../../shared/constants/echo-session';
import type { HelperClient } from '../helper';

export interface ClipboardSnapshot {
  readonly text: string;
  readonly html: string;
  readonly rtf: string;
  readonly imagePng: Uint8Array | null;
}

export interface ClipboardAdapter {
  snapshot(): ClipboardSnapshot;
  writeText(text: string): void;
  restore(snapshot: ClipboardSnapshot): void;
}

export interface PasteAdapter {
  injectPaste(
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<{ readonly submitted: boolean }>;
}

export const INSERTION_DISPATCH_TIMEOUT_MS = 3_500;
export const INSERTION_RESTORE_TIMEOUT_MS = 1_000;

export interface InsertionResult {
  readonly inserted: boolean;
  readonly copied: boolean;
  readonly cancelled?: boolean;
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
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<InsertionResult> {
    if (isAborted(signal)) return { inserted: false, copied: false, cancelled: true };
    const snapshot = this.#clipboard.snapshot();
    this.#clipboard.writeText(text);
    const ownershipFingerprint = clipboardFingerprint(this.#clipboard.snapshot());
    let submitted = false;
    let nativeCommitted = false;
    let commitPublished = false;
    const publishCommit = (): void => {
      nativeCommitted = true;
      if (commitPublished) return;
      commitPublished = true;
      onCommitted?.();
    };
    try {
      submitted = (
        await boundedOperation(
          this.#paste.injectPaste(signal, publishCommit),
          INSERTION_DISPATCH_TIMEOUT_MS,
        )
      ).submitted;
    } catch {
      submitted = false;
    }
    // The helper emits the irreversible boundary before its RPC response. Once observed, a late
    // false response, transport rejection, caller abort, or missing acknowledgement cannot turn an
    // actual native paste into cancellation.
    submitted ||= nativeCommitted;
    if (!submitted) {
      if (isAborted(signal)) {
        if (clipboardFingerprint(this.#clipboard.snapshot()) === ownershipFingerprint) {
          this.#clipboard.restore(snapshot);
        }
        return { inserted: false, copied: false, cancelled: true };
      }
      return { inserted: false, copied: true };
    }
    // Paste dispatch is the cancellation commit point. The caller records it before the
    // clipboard restoration delay so late Esc cannot misreport an already-inserted session.
    publishCommit();
    await boundedOperation(
      this.#delay(ECHO_CLIPBOARD_RESTORE_MS),
      INSERTION_RESTORE_TIMEOUT_MS,
    ).catch(() => undefined);
    // Once paste has been dispatched, cancellation must not strand our temporary
    // clipboard value. Restore only while it is still ours so a user clipboard
    // change during the delay always wins.
    if (clipboardFingerprint(this.#clipboard.snapshot()) === ownershipFingerprint) {
      try {
        this.#clipboard.restore(snapshot);
      } catch {
        // Paste dispatch is already committed. Restoration failure must not misreport the
        // insertion or let a later cancellation overwrite its terminal outcome.
      }
    }
    return { inserted: true, copied: false };
  }
}

export class ElectronClipboardAdapter implements ClipboardAdapter {
  snapshot(): ClipboardSnapshot {
    const image = clipboard.readImage();
    return {
      text: clipboard.readText(),
      html: clipboard.readHTML(),
      rtf: clipboard.readRTF(),
      imagePng: image.isEmpty() ? null : Uint8Array.from(image.toPNG()),
    };
  }

  writeText(text: string): void {
    clipboard.writeText(text);
  }

  restore(snapshot: ClipboardSnapshot): void {
    clipboard.write({
      text: snapshot.text,
      ...(snapshot.html.length === 0 ? {} : { html: snapshot.html }),
      ...(snapshot.rtf.length === 0 ? {} : { rtf: snapshot.rtf }),
      ...(snapshot.imagePng === null
        ? {}
        : { image: nativeImage.createFromBuffer(Buffer.from(snapshot.imagePng)) }),
    });
  }
}

export function clipboardFingerprint(snapshot: ClipboardSnapshot): string {
  const hash = createHash('sha256');
  updateFingerprintField(hash, 1, Buffer.from(snapshot.text, 'utf8'));
  updateFingerprintField(hash, 2, Buffer.from(snapshot.html, 'utf8'));
  updateFingerprintField(hash, 3, Buffer.from(snapshot.rtf, 'utf8'));
  updateFingerprintField(
    hash,
    4,
    snapshot.imagePng === null ? null : Buffer.from(snapshot.imagePng),
  );
  return hash.digest('hex');
}

function updateFingerprintField(
  hash: ReturnType<typeof createHash>,
  type: number,
  value: Buffer | null,
): void {
  const header = Buffer.allocUnsafe(10);
  header.writeUInt8(type, 0);
  header.writeUInt8(value === null ? 0 : 1, 1);
  header.writeBigUInt64BE(BigInt(value?.byteLength ?? 0), 2);
  hash.update(header);
  if (value !== null) hash.update(value);
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
