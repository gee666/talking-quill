import { open } from 'node:fs/promises';
import { ThirdPartyNoticesSchema } from '../../shared/schemas/info';

const MAX_NOTICES_BYTES = 2_000_000;

export class NoticesService {
  readonly #path: string;
  #readTask: Promise<string> | null = null;

  constructor(path: string) {
    this.#path = path;
  }

  read(): Promise<string> {
    const existing = this.#readTask;
    if (existing !== null) return existing;
    const task = this.#readOnce();
    this.#readTask = task;
    void task.catch(() => {
      if (this.#readTask === task) this.#readTask = null;
    });
    return task;
  }

  async #readOnce(): Promise<string> {
    const handle = await open(this.#path, 'r');
    try {
      const metadata = await handle.stat();
      if (!metadata.isFile() || metadata.size > MAX_NOTICES_BYTES) throw invalidNotices();
      const contents = Buffer.allocUnsafe(metadata.size + 1);
      let offset = 0;
      while (offset < contents.length) {
        const { bytesRead } = await handle.read(contents, offset, contents.length - offset, offset);
        if (bytesRead === 0) break;
        offset += bytesRead;
      }
      if (offset > metadata.size || offset > MAX_NOTICES_BYTES) throw invalidNotices();
      return ThirdPartyNoticesSchema.parse(contents.subarray(0, offset).toString('utf8'));
    } finally {
      await handle.close();
    }
  }
}

function invalidNotices(): Error {
  return new Error('Notices resource is invalid');
}
