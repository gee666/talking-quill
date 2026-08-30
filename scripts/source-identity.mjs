import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

const HEX_40 = /^[0-9a-f]{40}$/u;

export function currentSourceIdentity({
  repositoryRoot = resolve(import.meta.dirname, '..'),
  environment = process.env,
  requireClean = false,
} = {}) {
  const sourceCommit = git(repositoryRoot, ['rev-parse', 'HEAD^{commit}']);
  const sourceTree = git(repositoryRoot, ['rev-parse', 'HEAD^{tree}']);
  if (!HEX_40.test(sourceCommit) || !HEX_40.test(sourceTree)) {
    throw new Error('Unable to resolve the current source commit and tree');
  }
  const expectedCommit = environment.TALKING_QUILL_RELEASE_COMMIT;
  const expectedTree = environment.TALKING_QUILL_RELEASE_TREE;
  if (expectedCommit !== undefined && expectedCommit !== sourceCommit) {
    throw new Error('Checkout does not match TALKING_QUILL_RELEASE_COMMIT');
  }
  if (expectedTree !== undefined && expectedTree !== sourceTree) {
    throw new Error('Checkout does not match TALKING_QUILL_RELEASE_TREE');
  }
  if (requireClean || environment.TALKING_QUILL_REQUIRE_CLEAN_SOURCE === '1') {
    const status = git(repositoryRoot, [
      'status',
      '--porcelain=v1',
      '--untracked-files=normal',
      '--',
      '.',
    ]);
    if (status.length > 0) throw new Error('Release provenance requires a clean source tree');
  }
  return Object.freeze({ sourceCommit, sourceTree });
}

function git(repositoryRoot, arguments_) {
  return execFileSync('git', arguments_, {
    cwd: repositoryRoot,
    encoding: 'utf8',
    timeout: 30_000,
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
}
