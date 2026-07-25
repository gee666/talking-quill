import { mkdir, readdir, rm } from 'node:fs/promises';
import { join } from 'node:path';

export async function scavengeSessionArtifacts(
  directory: string,
  batchSize = 64,
  signal?: AbortSignal,
): Promise<void> {
  if (!Number.isSafeInteger(batchSize) || batchSize < 1)
    throw new Error('Session cleanup batch size is invalid');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const entries = await readdir(directory, { withFileTypes: true });
  for (let index = 0; index < entries.length; index += batchSize) {
    if (signal?.aborted === true) return;
    await Promise.all(
      entries.slice(index, index + batchSize).map((entry) =>
        rm(join(directory, entry.name), {
          recursive: entry.isDirectory(),
          force: true,
          maxRetries: 3,
        }),
      ),
    );
    await new Promise<void>((resolveWait) => setImmediate(resolveWait));
  }
}
