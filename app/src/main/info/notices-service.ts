import { readFile } from 'node:fs/promises';
import { ThirdPartyNoticesSchema } from '../../shared/schemas/info';

export class NoticesService {
  readonly #path: string;

  constructor(path: string) {
    this.#path = path;
  }

  async read(): Promise<string> {
    const metadata = await import('node:fs/promises').then(({ stat }) => stat(this.#path));
    if (!metadata.isFile() || metadata.size > 2_000_000)
      throw new Error('Notices resource is invalid');
    return ThirdPartyNoticesSchema.parse(await readFile(this.#path, 'utf8'));
  }
}
