import { createHash } from 'node:crypto';
import { copyFile, lstat, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { currentSourceIdentity } from './source-identity.mjs';
import {
  currentSourceTreeHash,
  validateArtifactProvenanceManifest,
} from './artifact-provenance.mjs';
import { sealReleaseManifest } from './release-manifest.mjs';
import { releaseConfig } from './release-config.mjs';
import { verifyCoordinatedVersions } from './release-version-policy.mjs';
import { verifyWindowsInstallerUiEvidence } from './windows-installer-ui-evidence.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');

export function validateStagedRelease(value, expected) {
  if (
    value.schemaVersion !== 1 ||
    value.result !== 'passed' ||
    value.packageMode !== 'fresh' ||
    value.variant !== 'canonical' ||
    Object.entries(expected).some(([key, content]) => value[key] !== content) ||
    !/^[0-9a-f]{64}$/u.test(value.tqpkg2TreeSha256)
  )
    throw new Error('Staged RELEASE.json does not match the exact fresh installer');
}

export async function assembleOrdinaryUnsignedRelease(preserved, smoke, output) {
  const root = resolve(import.meta.dirname, '..');
  const version = await verifyCoordinatedVersions(root);
  const identity = currentSourceIdentity({ requireClean: true });
  const sourceTreeSha256 = await currentSourceTreeHash();
  await mkdir(output, { recursive: true });
  if ((await readdir(output)).length !== 0) throw new Error('Release output must be empty');
  const provenance = [];
  let notices;
  for (const arch of ['x64', 'arm64']) {
    const directory = resolve(preserved, arch, 'tmp/release-upload');
    const installer = `Talking-Quill-${version}-win-${arch}-setup.exe`;
    const name = `provenance-win-${arch}-setup.json`;
    const names = [installer, name, 'RELEASE.json', 'THIRD_PARTY_NOTICES.txt'].sort();
    if (JSON.stringify((await readdir(directory)).sort()) !== JSON.stringify(names))
      throw new Error(`Staged ${arch} asset allowlist mismatch`);
    for (const file of names) {
      const stat = await lstat(resolve(directory, file));
      if (!stat.isFile() || stat.isSymbolicLink())
        throw new Error('Staged asset is not a regular file');
    }
    const bytes = await readFile(resolve(directory, installer));
    const value = JSON.parse(await readFile(resolve(directory, name), 'utf8'));
    validateArtifactProvenanceManifest(value);
    const final = value.entries.filter((entry) => entry.role === 'final-artifact');
    if (
      value.sourceCommit !== identity.sourceCommit ||
      value.sourceTree !== identity.sourceTree ||
      value.sourceTreeSha256 !== sourceTreeSha256 ||
      value.package.version !== version ||
      value.package.platform !== 'win' ||
      value.package.arch !== arch ||
      final.length !== 1 ||
      final[0].path.split('/').at(-1) !== installer ||
      final[0].sha256 !== hash(bytes)
    )
      throw new Error(`Staged ${arch} provenance does not match the source and installer`);
    validateStagedRelease(JSON.parse(await readFile(resolve(directory, 'RELEASE.json'), 'utf8')), {
      version,
      architecture: arch,
      ...identity,
      installer,
      bytes: bytes.length,
      sha256: hash(bytes),
    });
    const evidenceName = `windows-installer-ui-smoke-${arch}.json`;
    await verifyWindowsInstallerUiEvidence({
      evidencePath: resolve(smoke, evidenceName),
      installerPath: resolve(directory, installer),
      provenancePath: resolve(directory, name),
      architecture: arch,
    });
    for (const file of [installer, name])
      await copyFile(resolve(directory, file), resolve(output, file));
    await copyFile(resolve(directory, 'RELEASE.json'), resolve(output, `release-win-${arch}.json`));
    await copyFile(resolve(smoke, evidenceName), resolve(output, evidenceName));
    const architectureNotices = await readFile(resolve(directory, 'THIRD_PARTY_NOTICES.txt'));
    if (notices !== undefined && !notices.equals(architectureNotices))
      throw new Error('Architecture notices differ');
    notices = architectureNotices;
    provenance.push({
      name,
      platform: 'win',
      arch,
      mode: 'setup',
      sourceTree: identity.sourceTree,
      sourceTreeSha256,
    });
  }
  await writeFile(resolve(output, 'THIRD_PARTY_NOTICES.txt'), notices);
  const assets = [];
  for (const name of (await readdir(output)).sort()) {
    const bytes = await readFile(resolve(output, name));
    assets.push({ name, bytes: bytes.length, sha256: hash(bytes) });
  }
  const manifest = sealReleaseManifest({
    schemaVersion: 2,
    repository: releaseConfig.repository,
    tag: `v${version}`,
    version,
    ...identity,
    platform: 'win',
    architecture: 'x64+arm64',
    promotable: true,
    workflowRunId: process.env.GITHUB_RUN_ID ?? null,
    generatedAt: null,
    provenance,
    assets,
  });
  await writeFile(
    resolve(output, 'release-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  return manifest;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length !== 5)
    throw new Error('Usage: assemble-ordinary-unsigned-release PRESERVED SMOKE OUTPUT');
  await assembleOrdinaryUnsignedRelease(...process.argv.slice(2));
}
