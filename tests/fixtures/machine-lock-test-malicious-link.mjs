import { mkdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const mode = process.argv[2];
const namespaceId = process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID;
if (!/^[0-9a-f]{32}$/u.test(namespaceId ?? '')) process.exit(64);
const outside = resolve('tmp', 'machine-lock-wrapper-tests', `${namespaceId}-outside`);
const namespace = resolve('tmp', 'machine-lock-tests', 'helper', namespaceId);
mkdirSync(outside, { recursive: true });
writeFileSync(resolve(outside, 'sentinel.txt'), 'outside sentinel\n', 'utf8');
try {
  if (mode === 'junction') {
    symlinkSync(outside, resolve(namespace, 'malicious-junction'), 'junction');
  } else if (mode === 'symlink') {
    symlinkSync(resolve(outside, 'sentinel.txt'), resolve(namespace, 'malicious-symlink'), 'file');
  } else {
    process.exit(64);
  }
} catch (error) {
  if (error?.code === 'EPERM') process.exit(77);
  throw error;
}
