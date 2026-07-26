import { mkdir, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Protocol } from 'electron';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const electron = vi.hoisted(() => ({
  fetch: vi.fn<(url: string) => Promise<Response>>(),
}));

vi.mock('electron', () => ({
  net: { fetch: electron.fetch },
  protocol: { registerSchemesAsPrivileged: vi.fn() },
}));

import { installApplicationProtocol } from '../../app/src/main/security/protocol';

const owned: string[] = [];

afterEach(async () => {
  electron.fetch.mockReset();
  await Promise.all(owned.splice(0).map((path) => removeTestDirectory(path)));
});

function protocolHarness() {
  let handler: ((request: { readonly url: string }) => Promise<Response>) | null = null;
  const target = {
    handle: vi.fn((_scheme: string, next: typeof handler) => {
      handler = next;
    }),
    unhandle: vi.fn(),
  } as unknown as Protocol;
  return {
    target,
    request: async (url: string) => {
      if (handler === null) throw new Error('Protocol handler was not installed');
      return handler({ url });
    },
  };
}

describe('application protocol path policy', () => {
  it('serves a regular file contained by the canonical renderer root', async () => {
    const temporary = await createTestDirectory('protocol-contained');
    owned.push(temporary);
    const root = join(temporary, 'renderer');
    await mkdir(root);
    await writeFile(join(root, 'index.html'), '<main>safe</main>');
    electron.fetch.mockResolvedValueOnce(new Response('safe'));
    const test = protocolHarness();
    const dispose = installApplicationProtocol(root, test.target);

    const response = await test.request('talking-quill://app/index.html');
    expect(response.status).toBe(200);
    expect(electron.fetch).toHaveBeenCalledOnce();
    expect(response.headers.get('Content-Security-Policy')).toContain("default-src 'none'");
    dispose();
  });

  it('rejects a renderer-root junction or symlink that resolves outside the root', async () => {
    const temporary = await createTestDirectory('protocol-link-escape');
    owned.push(temporary);
    const root = join(temporary, 'renderer');
    const external = join(temporary, 'external');
    await mkdir(root);
    await mkdir(external);
    await writeFile(join(external, 'secret.txt'), 'must not be served');
    await symlink(
      external,
      join(root, 'linked'),
      process.platform === 'win32' ? 'junction' : 'dir',
    );
    const test = protocolHarness();
    installApplicationProtocol(root, test.target);

    const response = await test.request('talking-quill://app/linked/secret.txt');
    expect(response.status).toBe(404);
    expect(electron.fetch).not.toHaveBeenCalled();
  });
});
