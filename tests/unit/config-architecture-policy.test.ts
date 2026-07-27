import { readFile } from 'node:fs/promises';
import { posix } from 'node:path';
import { ESLint } from 'eslint';
import { describe, expect, it } from 'vitest';
import { POLICY_PATHS } from '../../scripts/verify-trusted-candidate.mjs';

const eslint = new ESLint();

interface RestrictedImportOptions {
  paths?: { name: string; importNames?: string[]; message?: string }[];
  patterns?: { group: string[]; message?: string }[];
}

async function restrictedImportOptions(filePath: string): Promise<RestrictedImportOptions> {
  const config = (await eslint.calculateConfigForFile(filePath)) as unknown;
  const rules =
    typeof config === 'object' && config !== null && 'rules' in config ? config.rules : undefined;
  const rule =
    typeof rules === 'object' && rules !== null
      ? (rules as Record<string, unknown>)['no-restricted-imports']
      : undefined;
  const options: unknown = Array.isArray(rule) ? rule[1] : undefined;
  if (typeof options !== 'object' || options === null) {
    throw new Error(`no-restricted-imports is not configured for ${filePath}`);
  }
  return options;
}

async function relativeModuleClosure(entryPaths: readonly string[]): Promise<Set<string>> {
  const dependencies = new Set<string>();
  const visited = new Set<string>();
  const pending = [...entryPaths];
  const relativeImport =
    /(?:\bfrom\s*|\bimport\s*(?:\(\s*)?|\brequire\s*\(\s*)["'](\.[^"']+)["']/gu;
  const nodeChildProcess =
    /(?:spawnSync|execFileSync)\(\s*process\.execPath\s*,\s*\[\s*["']([^"']+\.(?:mjs|cjs|js))["']/gu;
  while (pending.length > 0) {
    const modulePath = pending.pop();
    if (modulePath === undefined || visited.has(modulePath)) continue;
    visited.add(modulePath);
    const source = await readFile(modulePath, 'utf8');
    const discovered = [
      ...[...source.matchAll(relativeImport)].map((match) =>
        posix.normalize(posix.join(posix.dirname(modulePath), match[1] ?? '')),
      ),
      ...[...source.matchAll(nodeChildProcess)].map((match) => posix.normalize(match[1] ?? '')),
    ];
    for (const dependency of discovered) {
      if (dependency.length > 0 && !dependencies.has(dependency)) {
        dependencies.add(dependency);
        pending.push(dependency);
      }
    }
  }
  return dependencies;
}

describe('configuration architecture policy', () => {
  it('covers the relative module dependency closure of protected release executables', async () => {
    const protectedPaths = new Set(POLICY_PATHS);
    const closure = await relativeModuleClosure(
      POLICY_PATHS.filter((path) => /\.(?:mjs|cjs|js)$/u.test(path)),
    );
    expect([...closure].filter((path) => !protectedPaths.has(path)).sort()).toEqual([]);
    for (const dependency of [
      '.npmrc',
      'scripts/generate-notices.mjs',
      'scripts/native-architecture.mjs',
      'scripts/release-config.mjs',
      'scripts/secret-rules.mjs',
    ]) {
      expect(protectedPaths.has(dependency), dependency).toBe(true);
    }
  });

  it('retains renderer/shared and preload boundaries in narrow transport exceptions', async () => {
    const [sharedRule, preloadTransportRule] = await Promise.all([
      restrictedImportOptions('app/src/shared/ipc/registry.ts'),
      restrictedImportOptions('app/src/preload/transport.ts'),
    ]);
    expect(sharedRule.paths).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: 'electron' }),
        expect.objectContaining({
          name: 'node:fs',
          message: 'Renderer/shared code cannot import Node.',
        }),
      ]),
    );
    expect(sharedRule.patterns).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          group: ['node:*'],
          message: 'Renderer/shared code cannot import Node.',
        }),
        expect.objectContaining({
          group: ['**/main/**', '**/preload/**'],
          message: 'Renderer/shared boundary violation.',
        }),
      ]),
    );
    expect(preloadTransportRule.paths).toBeUndefined();
    expect(preloadTransportRule.patterns).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          group: ['**/main/**', '**/renderer/**'],
          message: 'Preload boundary violation.',
        }),
      ]),
    );
  });

  it('typechecks renderer sources without ambient Node globals', async () => {
    const [rendererConfigSource, manifestSource] = await Promise.all([
      readFile('app/tsconfig.renderer.json', 'utf8'),
      readFile('app/package.json', 'utf8'),
    ]);
    const rendererConfig = JSON.parse(rendererConfigSource) as {
      compilerOptions: { types: string[] };
    };
    const manifest = JSON.parse(manifestSource) as { scripts: Record<string, string> };
    expect(rendererConfig.compilerOptions.types).toEqual(['vite/client']);
    expect(manifest.scripts.typecheck).toContain('tsconfig.renderer.json');
  });

  it('uses the configured product filename in every macOS after-pack path', async () => {
    const hook = await readFile('app/after-pack.cjs', 'utf8');
    expect(hook).toContain('context.packager.appInfo.productFilename');
    expect(hook).not.toContain('Talking Quill.app/Contents/Resources');
  });
});
