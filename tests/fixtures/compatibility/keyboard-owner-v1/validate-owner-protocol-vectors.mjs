import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const directory = resolve(import.meta.dirname);
for (const [name, expectedCount] of [
  ['owner-protocol-vectors.json', 1],
  ['release-policy-vectors.json', 12],
]) {
  const value = JSON.parse(readFileSync(resolve(directory, name), 'utf8'));
  if (value.fixtureVersion !== 1 || !Array.isArray(value.vectors)) {
    throw new Error(`Invalid compatibility fixture: ${name}`);
  }
  if (value.vectors.length !== expectedCount) {
    throw new Error(`Unexpected compatibility vector count: ${name}`);
  }
  const names = value.vectors.map((vector) => vector?.id ?? vector?.name);
  if (
    names.some((vectorName) => typeof vectorName !== 'string') ||
    new Set(names).size !== names.length
  ) {
    throw new Error(`Compatibility vector names are invalid: ${name}`);
  }
}
