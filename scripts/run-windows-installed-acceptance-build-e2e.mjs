import { createHash, randomBytes } from 'node:crypto';
import { lstat, readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { buildWindowsInstalledAcceptanceInputs } from './build-windows-installed-acceptance-inputs.mjs';
import { verifyAcceptanceBundleArchive } from './windows-installed-acceptance-bundle.mjs';
import { MAX_ACCEPTANCE_RUN_MS } from './windows-installed-acceptance.mjs';

if (process.platform !== 'win32' || process.arch !== 'x64') {
  throw new Error('The installed-acceptance build E2E requires native Windows x64');
}
const required = (name) => {
  const value = process.env[name];
  if (value === undefined || value.length === 0) throw new Error(`Missing E2E input: ${name}`);
  return value;
};
const roots = [
  resolve(process.env.ProgramW6432 ?? 'C:/Program Files', 'Talking Quill'),
  resolve(process.env.ProgramData ?? 'C:/ProgramData', 'Talking Quill'),
  resolve(process.env.ProgramData ?? 'C:/ProgramData', 'Talking Quill Acceptance Native'),
];
const before = await Promise.all(roots.map(treeHash));
const notBeforeMs = Date.now() + 5 * 60_000;
const result = await buildWindowsInstalledAcceptanceInputs({
  architecture: 'x64',
  descriptorPath: required('TQ_ACCEPTANCE_E2E_RELEASE'),
  descriptorSha256: required('TQ_ACCEPTANCE_E2E_RELEASE_SHA256'),
  provenancePath: required('TQ_ACCEPTANCE_E2E_PROVENANCE'),
  provenanceSha256: required('TQ_ACCEPTANCE_E2E_PROVENANCE_SHA256'),
  sourceRoot: resolve('.'),
  requestPrivateKeyPath: required('TQ_ACCEPTANCE_E2E_REQUEST_KEY'),
  manifestPrivateKeyPath: required('TQ_ACCEPTANCE_E2E_MANIFEST_KEY'),
  updatePrivateKeyPath: required('TQ_ACCEPTANCE_E2E_UPDATE_KEY'),
  validationPrivateKeyPath: required('TQ_ACCEPTANCE_E2E_VALIDATION_KEY'),
  signerPath: resolve(
    'helper/target/x86_64-pc-windows-msvc/release/talking-quill-acceptance-signer.exe',
  ),
  signerSha256: '0'.repeat(64),
  notBeforeMs,
  expiresAtMs: notBeforeMs + MAX_ACCEPTANCE_RUN_MS,
  buildId: randomBytes(32).toString('hex'),
});
if (result.kit === null || typeof result.kit.bundlePath !== 'string') {
  throw new Error('Installed-acceptance E2E did not produce a bundle');
}
await verifyAcceptanceBundleArchive(result.kit.bundlePath, { architecture: 'x64' });
const after = await Promise.all(roots.map(treeHash));
if (JSON.stringify(before) !== JSON.stringify(after)) {
  throw new Error('Installed-acceptance build E2E changed production machine state');
}
console.log(
  JSON.stringify({
    result: 'passed',
    bundleSha256: result.kit.bundleSha256,
    producerArtifactSetIdentity: result.kit.producerArtifactSetIdentity,
  }),
);

async function treeHash(path) {
  const digest = createHash('sha256');
  const root = await lstat(path).catch((error) => {
    if (error?.code === 'ENOENT') return null;
    throw error;
  });
  if (root === null) return digest.update('absent').digest('hex');
  if (!root.isDirectory() || root.isSymbolicLink()) {
    throw new Error('Production snapshot root is not a directory');
  }
  digest.update('directory');
  const visit = async (directory, prefix) => {
    const entries = await readdir(directory, { withFileTypes: true }).catch((error) => {
      if (error?.code === 'ENOENT') return [];
      throw error;
    });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const absolute = resolve(directory, entry.name);
      const local = `${prefix}/${entry.name}`;
      const metadata = await lstat(absolute);
      if (metadata.isSymbolicLink()) throw new Error('Production snapshot contains a link');
      digest.update(local).update(String(metadata.size));
      if (metadata.isDirectory()) await visit(absolute, local);
      else if (metadata.isFile()) digest.update(await readFile(absolute));
    }
  };
  await visit(path, '');
  return digest.digest('hex');
}
