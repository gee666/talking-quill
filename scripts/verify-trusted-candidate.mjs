import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const POLICY_PATHS = Object.freeze([
  '.npmrc',
  'release.config.json',
  'package.json',
  'pnpm-lock.yaml',
  'pnpm-workspace.yaml',
  'app/package.json',
  'app/after-pack.cjs',
  'build/electron-builder.yml',
  'build/installer.nsh',
  'build/mac-sign.cjs',
  'scripts/release-control-preflight.mjs',
  'scripts/verify-trusted-candidate.mjs',
  'scripts/verify-release-tag.mjs',
  'scripts/artifact-provenance.mjs',
  'scripts/assemble-release.mjs',
  'scripts/release-checksums.mjs',
  'scripts/verify-draft-release.mjs',
  'scripts/verify-public-release.mjs',
  'scripts/verify-macos-release.mjs',
  'scripts/generate-notices.mjs',
  'scripts/inspect-package.mjs',
  'scripts/native-architecture.mjs',
  'scripts/package-policy.mjs',
  'scripts/release-config.mjs',
  'scripts/secret-rules.mjs',
]);
function hashGitPaths(repository, paths) {
  const tree = execFileSync('git', ['ls-tree', 'HEAD', '--', ...paths], {
    cwd: repository,
    encoding: 'utf8',
  });
  const actual = tree.trim().split(/\r?\n/u).filter(Boolean);
  if (actual.length !== paths.length) throw new Error('Protected release path set is incomplete.');
  return createHash('sha256').update(tree.replace(/\r\n/gu, '\n')).digest('hex');
}

export function parseApprovedFingerprints(value) {
  const fingerprints = value
    .split(/[\s,]+/u)
    .filter(Boolean)
    .map((item) => item.toUpperCase());
  if (fingerprints.length === 0 || fingerprints.some((item) => !/^[0-9A-F]{40,64}$/u.test(item))) {
    throw new Error('Protected release signer fingerprints are missing or invalid.');
  }
  return new Set(fingerprints);
}

function requireHash(name) {
  const value = process.env[name]?.toLowerCase();
  if (!/^[0-9a-f]{64}$/u.test(value ?? '')) throw new Error(`${name} is missing or invalid.`);
  return value;
}

function runGit(repository, args, environment = process.env) {
  const result = spawnSync('git', args, { cwd: repository, env: environment, encoding: 'utf8' });
  if (result.status !== 0)
    throw new Error(`Trusted candidate git check failed: git ${args.join(' ')}`);
  return `${result.stdout}${result.stderr}`;
}

function main() {
  const arguments_ = process.argv.slice(2).filter((value) => value !== '--');
  if (arguments_[0] === '--print-protected-hashes') {
    const repository = resolve(arguments_[1] ?? '.');
    console.log(
      JSON.stringify(
        {
          RELEASE_POLICY_TREE_SHA256: hashGitPaths(repository, POLICY_PATHS),
        },
        null,
        2,
      ),
    );
    return;
  }
  const [rawTag, repositoryArgument = '.'] = arguments_;
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.test(rawTag ?? '')) {
    throw new Error('Trusted candidate requires a strict semver tag.');
  }
  const repository = resolve(repositoryArgument);
  const defaultBranch = process.env.RELEASE_DEFAULT_BRANCH;
  if (!defaultBranch || !/^[A-Za-z0-9._/-]+$/u.test(defaultBranch)) {
    throw new Error('RELEASE_DEFAULT_BRANCH is missing or invalid.');
  }
  const publicKey = process.env.RELEASE_TAG_SIGNING_PUBLIC_KEY;
  if (!publicKey) throw new Error('Protected release signing public key is missing.');
  const approved = parseApprovedFingerprints(process.env.RELEASE_TAG_SIGNER_FINGERPRINTS ?? '');
  if (runGit(repository, ['status', '--porcelain=v1', '--untracked-files=normal']).trim()) {
    throw new Error('Candidate checkout contains modifications before trust verification.');
  }
  if (hashGitPaths(repository, POLICY_PATHS) !== requireHash('RELEASE_POLICY_TREE_SHA256')) {
    throw new Error('Candidate release policy differs from the protected policy hash.');
  }
  const home = mkdtempSync(resolve(tmpdir(), 'talking-quill-release-gnupg-'));
  try {
    mkdirSync(home, { recursive: true, mode: 0o700 });
    const keyPath = resolve(home, 'release-key.asc');
    writeFileSync(keyPath, publicKey, { encoding: 'utf8', mode: 0o600 });
    const environment = { ...process.env, GNUPGHOME: home };
    const importResult = spawnSync(
      'gpg',
      ['--batch', '--with-colons', '--import-options', 'show-only', '--import', keyPath],
      { env: environment, encoding: 'utf8' },
    );
    if (importResult.status !== 0) throw new Error('Protected release public key is invalid.');
    const keyFingerprints = [
      ...importResult.stdout.matchAll(/^fpr:::::::::([0-9A-F]{40,64}):$/gimu),
    ].map((match) => match[1].toUpperCase());
    if (!keyFingerprints.some((fingerprint) => approved.has(fingerprint))) {
      throw new Error('Protected public key does not match an approved signer fingerprint.');
    }
    const imported = spawnSync('gpg', ['--batch', '--import', keyPath], {
      env: environment,
      encoding: 'utf8',
    });
    if (imported.status !== 0) throw new Error('Protected release public key import failed.');
    if (
      runGit(repository, ['cat-file', '-t', `refs/tags/${rawTag}`], environment).trim() !== 'tag'
    ) {
      throw new Error('Trusted release candidate tag is not annotated.');
    }
    const verification = runGit(repository, ['verify-tag', '--raw', rawTag], environment);
    const signer = /\[GNUPG:\] VALIDSIG ([0-9A-F]{40,64})\b/iu
      .exec(verification)?.[1]
      ?.toUpperCase();
    if (!signer || !approved.has(signer)) {
      throw new Error('Trusted release candidate tag has an unapproved signature.');
    }
    const commit = runGit(repository, ['rev-parse', `${rawTag}^{commit}`], environment).trim();
    const head = runGit(repository, ['rev-parse', 'HEAD^{commit}'], environment).trim();
    if (commit !== head) throw new Error('Trusted tag does not resolve to the candidate checkout.');
    const protectedHead = runGit(
      repository,
      ['rev-parse', `origin/${defaultBranch}^{commit}`],
      environment,
    ).trim();
    if (commit !== protectedHead) {
      throw new Error('Trusted release candidate must equal the protected default-branch head.');
    }
    if (process.env.GITHUB_OUTPUT) {
      writeFileSync(
        process.env.GITHUB_OUTPUT,
        `tag=${rawTag}\nversion=${rawTag.slice(1)}\ncommit=${commit}\nnotes_path=docs/releases/${rawTag}-release-notes.md\nrepository=${process.env.GITHUB_REPOSITORY ?? ''}\npolicy_sha256=${hashGitPaths(repository, POLICY_PATHS)}\n`,
        { flag: 'a' },
      );
    }
    console.log(
      `Trusted signed candidate ${rawTag} equals protected origin/${defaultBranch} at ${commit}.`,
    );
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
