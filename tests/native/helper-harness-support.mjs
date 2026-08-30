import { copyFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

import {
  nativeRoleLayout,
  verifyNativeSourceIdentity,
  verifyStagedNativeRoleSet,
} from '../../scripts/helper-build-contract.mjs';
import {
  RELEASE_PACKAGE_METADATA_NAME,
  createPackageReleaseMetadata,
} from '../../scripts/release-package-metadata.mjs';
import { currentSourceIdentity } from '../../scripts/source-identity.mjs';

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

  const sourceIdentity = currentSourceIdentity({ repositoryRoot, requireClean: true });
  await verifyStagedNativeRoleSet(sourceDirectory, { platform, architecture });
  for (const role of nativeRoleLayout(platform)) {
    await verifyNativeSourceIdentity(join(sourceDirectory, role.name), sourceIdentity);
  }

  const packageRoot = resolve(repositoryRoot, 'tmp', `helper-harness-runtime-${processId}`);
  const helperDirectory = join(packageRoot, 'resources', 'helper');
  await rm(packageRoot, { recursive: true, force: true });
  await mkdir(helperDirectory, { recursive: true });
  for (const role of nativeRoleLayout(platform)) {
    await copyFile(join(sourceDirectory, role.name), join(helperDirectory, role.name));
  }
  const packageJson = JSON.parse(await readFile(join(repositoryRoot, 'package.json'), 'utf8'));
  const metadata = await createPackageReleaseMetadata({
    version: packageJson.version,
    platform: 'win',
    architecture,
    packageRoot,
    predecessor: null,
    sourceIdentity,
    freshInstall: true,
    packageMode: 'fresh',
  });
  await writeFile(
    join(packageRoot, 'resources', RELEASE_PACKAGE_METADATA_NAME),
    `${JSON.stringify(metadata)}\n`,
    { encoding: 'utf8', mode: 0o600 },
  );
  return {
    executable: join(helperDirectory, 'talking-quill-helper.exe'),
    staged: true,
    cleanup: () =>
      rm(packageRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }),
  };
}
