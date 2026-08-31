import { createHash, randomUUID } from 'node:crypto';
import { lstat, mkdir, open, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

import {
  SOURCE_COMMIT_MARKER,
  SOURCE_TREE_MARKER,
  nativeRoleLayout,
  verifyNativeSourceIdentity,
  verifyStagedNativeRoleSet,
} from '../../scripts/helper-build-contract.mjs';
import {
  RELEASE_PACKAGE_METADATA_NAME,
  createPackageReleaseMetadata,
  validatePackageReleaseMetadata,
} from '../../scripts/release-package-metadata.mjs';

export const FAILURE_CLEANUP_REQUESTS = Object.freeze([
  Object.freeze(['session.set_capture', Object.freeze({ mode: 'off' })]),
  Object.freeze([
    'activation.configure',
    Object.freeze({ enabled: false, bindings: Object.freeze([]) }),
  ]),
  Object.freeze(['shutdown', Object.freeze({})]),
]);

export async function prepareHelperHarnessExecutable({
  helper,
  repositoryRoot,
  platform = process.platform,
  architecture = process.arch,
  processId = process.pid,
}) {
  const sourceDirectory = resolve(repositoryRoot, 'app', 'native');
  if (
    platform !== 'win32' ||
    dirname(helper).toLocaleLowerCase('en-US') !== sourceDirectory.toLocaleLowerCase('en-US')
  ) {
    return { executable: helper, cleanup: async () => undefined, staged: false };
  }

  await verifyStagedNativeRoleSet(sourceDirectory, { platform, architecture });

  const random = randomUUID();
  const packageRoot = resolve(
    repositoryRoot,
    'tmp',
    `helper-harness-runtime-${processId}-${random}`,
  );
  const pendingRoot = `${packageRoot}.pending`;
  const pendingHelperDirectory = join(pendingRoot, 'resources', 'helper');
  try {
    await mkdir(pendingHelperDirectory, { recursive: true });
    const layout = nativeRoleLayout(platform);
    const retained = [];
    let sourceIdentity;
    try {
      for (const role of layout) {
        retained.push(await open(join(sourceDirectory, role.name), 'r'));
      }
      for (const [index, role] of layout.entries()) {
        const bytes = await retained[index].readFile();
        const identity = retainedSourceIdentity(bytes, role.name);
        if (sourceIdentity === undefined) sourceIdentity = identity;
        else if (
          identity.sourceCommit !== sourceIdentity.sourceCommit ||
          identity.sourceTree !== sourceIdentity.sourceTree
        ) {
          throw new Error('Retained gateway and owner source identities differ');
        }
        await writeFile(join(pendingHelperDirectory, role.name), bytes, {
          flag: 'wx',
          mode: 0o700,
        });
      }
    } finally {
      await Promise.all(retained.map((handle) => handle.close()));
    }
    if (sourceIdentity === undefined) throw new Error('Native harness role set is empty');
    await verifyStagedNativeRoleSet(pendingHelperDirectory, { platform, architecture });
    for (const role of nativeRoleLayout(platform)) {
      await verifyNativeSourceIdentity(join(pendingHelperDirectory, role.name), sourceIdentity);
    }
    const packageJson = JSON.parse(await readFile(join(repositoryRoot, 'package.json'), 'utf8'));
    const metadata = await createPackageReleaseMetadata({
      version: packageJson.version,
      platform: 'win',
      architecture,
      packageRoot: pendingRoot,
      predecessor: null,
      sourceIdentity,
      freshInstall: true,
      packageMode: 'fresh',
    });
    const metadataPath = join(pendingRoot, 'resources', RELEASE_PACKAGE_METADATA_NAME);
    await writeFile(metadataPath, `${JSON.stringify(metadata)}\n`, {
      encoding: 'utf8',
      mode: 0o600,
    });
    const serialized = await readFile(metadataPath);
    const reparsed = validatePackageReleaseMetadata(JSON.parse(serialized.toString('utf8')));
    if (`${JSON.stringify(reparsed)}\n` !== serialized.toString('utf8')) {
      throw new Error('Serialized harness package metadata is not canonical');
    }
    for (const role of reparsed.roles) {
      const bytes = await readFile(join(pendingRoot, role.path));
      if (createHash('sha256').update(bytes).digest('hex') !== role.sha256) {
        throw new Error(`Serialized harness role hash mismatch: ${role.role}`);
      }
    }
    await rename(pendingRoot, packageRoot);
    const executable = join(packageRoot, 'resources', 'helper', 'talking-quill-helper.exe');
    return {
      executable,
      ownerExecutable: join(packageRoot, 'resources', 'helper', 'talking-quill-keyboard-owner.exe'),
      packageRoot,
      staged: true,
      cleanup: async () => {
        await rm(packageRoot, { recursive: true, force: true, maxRetries: 50, retryDelay: 100 });
        try {
          await lstat(packageRoot);
          throw new Error(`Native harness staging root remains after cleanup: ${packageRoot}`);
        } catch (error) {
          if (error?.code !== 'ENOENT') throw error;
        }
      },
    };
  } catch (error) {
    await Promise.all([
      rm(pendingRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }),
      rm(packageRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }),
    ]);
    throw error;
  }
}

function retainedSourceIdentity(bytes, role) {
  const identity = {};
  for (const [marker, field, label] of [
    [SOURCE_COMMIT_MARKER, 'sourceCommit', 'commit'],
    [SOURCE_TREE_MARKER, 'sourceTree', 'tree'],
  ]) {
    const offset = bytes.indexOf(marker);
    const value = bytes.subarray(offset + marker.length, offset + marker.length + 40);
    if (
      offset < 0 ||
      bytes.indexOf(marker, offset + 1) >= 0 ||
      !/^[0-9a-f]{40}$/u.test(value.toString('ascii'))
    ) {
      throw new Error(`Retained ${role} source ${label} marker is invalid`);
    }
    identity[field] = value.toString('ascii');
  }
  return identity;
}
