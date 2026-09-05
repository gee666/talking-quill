import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';

it('keeps audio and Echo runtime imports acyclic, allowing type-only dependencies', async () => {
  const directories = ['audio', 'echo'].map((name) => resolve('app/src/main', name));
  const files = (
    await Promise.all(
      directories.map(async (directory) =>
        (await readdir(directory))
          .filter((name) => name.endsWith('.ts'))
          .map((name) => resolve(directory, name)),
      ),
    )
  ).flat();
  const graph = new Map<string, string[]>();
  for (const file of files) {
    // Inspect emitted imports so mixed imports and TypeScript's erased types are handled alike.
    const { outputText } = ts.transpileModule(await readFile(file, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ESNext },
    });
    const source = ts.createSourceFile(file, outputText, ts.ScriptTarget.ESNext, true);
    const dependencies: string[] = [];
    for (const statement of source.statements) {
      if (!ts.isImportDeclaration(statement) && !ts.isExportDeclaration(statement)) continue;
      const specifier = statement.moduleSpecifier;
      if (specifier === undefined || !ts.isStringLiteral(specifier)) continue;
      if (!specifier.text.startsWith('.')) continue;
      dependencies.push(resolve(dirname(file), `${specifier.text}.ts`));
    }
    graph.set(file, dependencies);
  }
  const visited = new Set<string>();
  const visiting: string[] = [];
  const visit = (file: string): void => {
    expect(visiting, `Runtime cycle: ${[...visiting, file].join(' -> ')}`).not.toContain(file);
    if (visited.has(file)) return;
    visiting.push(file);
    for (const dependency of graph.get(file) ?? []) {
      if (graph.has(dependency)) visit(dependency);
    }
    visiting.pop();
    visited.add(file);
  };
  for (const file of files) visit(file);
});
