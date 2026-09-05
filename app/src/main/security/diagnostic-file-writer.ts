import { chmod, open, rename, rm, stat } from 'node:fs/promises';
import { join } from 'node:path';

/** Shares one temporary-file sequence across ordinary logs and owner checkpoints. */
export class DiagnosticFileWriter {
  readonly #directory: string;
  readonly #path: string;
  readonly #maxBytes: number;
  readonly #retainedFiles: number;
  readonly #now: () => number;
  #temporarySequence = 0;

  constructor(directory: string, maxBytes: number, retainedFiles: number, now: () => number) {
    this.#directory = directory;
    this.#path = join(directory, 'diagnostic.jsonl');
    this.#maxBytes = maxBytes;
    this.#retainedFiles = retainedFiles;
    this.#now = now;
  }

  async atomicReplace(contents: Buffer): Promise<void> {
    await this.atomicReplaceAt(this.#path, contents);
  }

  async atomicReplaceAt(target: string, contents: Buffer): Promise<void> {
    const temporary = join(
      this.#directory,
      `.diagnostic-${String(process.pid)}-${String(this.#now())}-${String((this.#temporarySequence += 1))}.tmp`,
    );
    let handle: Awaited<ReturnType<typeof open>> | null = null;
    try {
      handle = await open(temporary, 'wx', 0o600);
      await handle.writeFile(contents);
      await handle.sync();
      await handle.close();
      handle = null;
      await rename(temporary, target);
      const committed = await open(target, 'r+');
      try {
        await committed.sync();
      } finally {
        await committed.close();
      }
      await chmod(target, 0o600).catch(() => undefined);
    } finally {
      await handle?.close().catch(() => undefined);
      await rm(temporary, { force: true }).catch(() => undefined);
    }
  }

  async rotateIfNeeded(incomingBytes: number): Promise<void> {
    const currentBytes = await stat(this.#path).then(
      (value) => value.size,
      (error: unknown) => {
        if (isNodeError(error) && error.code === 'ENOENT') return 0;
        throw error;
      },
    );
    if (currentBytes + incomingBytes <= this.#maxBytes) return;
    await rm(`${this.#path}.${String(this.#retainedFiles)}`, { force: true });
    for (let index = this.#retainedFiles - 1; index >= 1; index -= 1) {
      await rename(`${this.#path}.${String(index)}`, `${this.#path}.${String(index + 1)}`).catch(
        (error: unknown) => {
          if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
        },
      );
    }
    await rename(this.#path, `${this.#path}.1`).catch((error: unknown) => {
      if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    });
  }
}

export function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
