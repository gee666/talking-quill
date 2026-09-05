import { existsSync, readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';

/** Include explicitly declared file modules when auditing a Rust entry point. */
function modulePaths(path: string, source: string): string[] {
  const directory = basename(path) === 'mod.rs' ? dirname(path) : path.replace(/\.rs$/u, '');
  const modules = [...source.matchAll(/^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;/gmu)];
  return modules.map((match) => {
    const name = match[1] ?? '';
    const file = join(directory, `${name}.rs`);
    return existsSync(file) ? file : join(directory, name, 'mod.rs');
  });
}

export function readRustModuleSync(path: string): string {
  const source = readFileSync(path, 'utf8');
  return [source, ...modulePaths(path, source).map(readRustModuleSync)].join('\n');
}

export async function readRustModule(path: string): Promise<string> {
  const source = await readFile(path, 'utf8');
  return [source, ...(await Promise.all(modulePaths(path, source).map(readRustModule)))].join('\n');
}
