import { createHash } from 'node:crypto';
import { lstat, readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { hostedInstallerBinding } from './windows-hosted-lifecycle-evidence.mjs';

export async function verifyHostedRuntimeTree(root, binding) {
  const names = [];
  async function walk(directory, prefix = '') {
    const directoryStat = await lstat(directory);
    if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink())
      throw new Error('Runtime directory must not be a reparse point');
    for (const name of await readdir(directory)) {
      const relative = `${prefix}${name}`;
      const path = resolve(directory, name);
      const stat = await lstat(path);
      if (stat.isSymbolicLink()) throw new Error(`Runtime reparse point: ${relative}`);
      if (stat.isDirectory()) await walk(path, `${relative}/`);
      else if (stat.isFile()) names.push(relative);
      else throw new Error(`Runtime entry is not a regular file: ${relative}`);
    }
  }
  await walk(resolve(root));
  const expected = binding.files.map((file) => file.path).sort();
  if (JSON.stringify(names.sort()) !== JSON.stringify(expected))
    throw new Error('Runtime file inventory differs from the installer payload');
  for (const file of binding.files) {
    const bytes = await readFile(resolve(root, file.path));
    if (
      bytes.length !== file.size ||
      createHash('sha256').update(bytes).digest('hex') !== file.sha256
    )
      throw new Error(`Runtime file differs from installer payload: ${file.path}`);
  }
  return { fileCount: names.length };
}

export function validateHostedRuntimeEvidence(value, binding) {
  const fields = [
    'architecture',
    'sourceCommit',
    'sourceTree',
    'sourceTreeSha256',
    'version',
    'installer',
    'installerSha256',
    'bytes',
    'provenanceDocumentSha256',
  ];
  if (
    value.schemaVersion !== 1 ||
    value.kind !== 'github-hosted-native-runtime' ||
    value.result !== 'passed' ||
    fields.some((key) => value[key] !== binding[key]) ||
    !/^[1-9][0-9]*$/u.test(value.workflowRunId) ||
    value.host !== 'github-hosted' ||
    value.coverage?.installation !== 'not-exercised' ||
    value.coverage.uac !== 'not-exercised' ||
    value.coverage.installerPayload !== 'verified' ||
    value.coverage.nativeRuntime !== 'exercised' ||
    value.runtimeFileCount !== binding.files.length ||
    value.runtimeVerifiedBefore !== true ||
    value.runtimeVerifiedAfter !== true ||
    value.lifecycle?.result !== 'passed' ||
    value.lifecycle.architecture !== binding.architecture ||
    value.lifecycle.mode !== 'unpacked' ||
    value.lifecycle.first?.result !== 'passed' ||
    value.lifecycle.successor?.result !== 'passed' ||
    value.lifecycle.crash?.ownerAuthenticated !== true ||
    value.lifecycle.ownershipCoverage?.authoritative !== false ||
    'installExitCode' in value ||
    'uninstallExitCode' in value
  ) {
    throw new Error(
      'Hosted runtime evidence does not prove the exact payload and native lifecycle coverage',
    );
  }
  return value;
}

export async function verifyHostedRuntimeEvidence({
  evidencePath,
  installerPath,
  provenancePath,
  architecture,
}) {
  const binding = await hostedInstallerBinding(installerPath, provenancePath, architecture);
  return validateHostedRuntimeEvidence(JSON.parse(await readFile(evidencePath, 'utf8')), binding);
}
