import { rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyCanonicalMainGraph } from './canonical-main-graph.mjs';
import { normalizeEnvironment } from './environment-policy.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const environment = normalizeEnvironment(process.env);
if (environment.TALKING_QUILL_PACKAGE_VARIANT === 'packaged-test') {
  throw new Error('Production build rejects the packaged-test variant');
}
if (environment.TALKING_QUILL_PACKAGE_VARIANT !== 'installed-acceptance') {
  await verifyCanonicalMainGraph();
}
rmSync(resolve(root, 'app/out'), { recursive: true, force: true });
const poisoned = Object.keys(environment).filter((name) => {
  const normalizedName = name.toUpperCase();
  return (
    /^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(normalizedName) &&
    environment[name] !== '' &&
    environment[name] !== '0'
  );
});
if (poisoned.length > 0) {
  throw new Error(
    `Production build rejects test-harness environment: ${poisoned.sort().join(', ')}`,
  );
}
