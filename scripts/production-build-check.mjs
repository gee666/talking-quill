import { rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
rmSync(resolve(root, 'app/out'), { recursive: true, force: true });
const poisoned = Object.keys(process.env).filter(
  (name) =>
    /^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(name) &&
    process.env[name] !== '' &&
    process.env[name] !== '0',
);
if (poisoned.length > 0) {
  throw new Error(
    `Production build rejects test-harness environment: ${poisoned.sort().join(', ')}`,
  );
}
