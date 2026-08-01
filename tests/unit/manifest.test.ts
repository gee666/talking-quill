import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

interface Manifest {
  readonly packageManager?: string;
  readonly dependencies?: Readonly<Record<string, string>>;
  readonly devDependencies?: Readonly<Record<string, string>>;
}

async function readManifest(path: string): Promise<Manifest> {
  return JSON.parse(await readFile(path, 'utf8')) as Manifest;
}

describe('reproducible dependency manifests', () => {
  it('pins the package manager and every direct dependency exactly', async () => {
    const root = await readManifest('package.json');
    const app = await readManifest('app/package.json');
    expect(root.packageManager).toBe('pnpm@11.17.0');
    for (const dependencies of [
      root.dependencies,
      root.devDependencies,
      app.dependencies,
      app.devDependencies,
    ]) {
      for (const version of Object.values(dependencies ?? {})) {
        expect(version).not.toMatch(/^[~^*]|latest|workspace:/);
        expect(version).toMatch(/^\d+\.\d+\.\d+(?:[-+].+)?$/);
      }
    }
  });
});
