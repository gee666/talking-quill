import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';
import { hostedInstallerBinding } from './windows-hosted-lifecycle-evidence.mjs';
import {
  validateHostedRuntimeEvidence,
  verifyHostedRuntimeTree,
} from './windows-hosted-runtime-evidence.mjs';

const [architecture, installerPath, provenancePath, runtimeRoot, outputInput] =
  process.argv.slice(2);
if (!outputInput || process.argv.length !== 7)
  throw new Error(
    'Usage: windows-hosted-runtime-smoke ARCH INSTALLER PROVENANCE RUNTIME_ROOT OUTPUT',
  );
const output = resolve(outputInput);
const insideTmp = relative(resolve('tmp'), output);
if (!insideTmp || insideTmp.startsWith('..') || isAbsolute(insideTmp))
  throw new Error('Runtime logs must remain under project tmp');
await mkdir(output, { recursive: true });
try {
  if (
    process.platform !== 'win32' ||
    process.env.GITHUB_ACTIONS !== 'true' ||
    process.env.RUNNER_ENVIRONMENT !== 'github-hosted' ||
    process.env.RUNNER_OS !== 'Windows' ||
    !/^[1-9][0-9]*$/u.test(process.env.GITHUB_RUN_ID ?? '') ||
    !['x64', 'arm64'].includes(architecture) ||
    process.arch !== architecture
  ) {
    throw new Error('Runtime smoke requires an architecture-native GitHub-hosted Windows runner');
  }
  const native = spawnSync(
    'pwsh',
    [
      '-NoProfile',
      '-Command',
      '[Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()',
    ],
    { encoding: 'utf8', timeout: 15_000 },
  );
  if (native.status !== 0 || native.stdout.trim() !== architecture)
    throw new Error('Runtime smoke must execute on the real native OS architecture');
  const binding = await hostedInstallerBinding(installerPath, provenancePath, architecture);
  await writeFile(resolve(output, 'binding.json'), JSON.stringify(binding));
  const before = await verifyHostedRuntimeTree(runtimeRoot, binding);
  const profileTemp = resolve(output, 'runtime-profiles');
  await mkdir(profileTemp, { recursive: true });
  const lifecycle = spawnSync(
    process.execPath,
    [
      'scripts/windows-package-lifecycle.mjs',
      '--arch',
      architecture,
      '--mode',
      'unpacked',
      '--root',
      resolve(runtimeRoot),
    ],
    {
      encoding: 'utf8',
      timeout: 300_000,
      maxBuffer: 10 * 1024 * 1024,
      env: { ...process.env, TEMP: profileTemp, TMP: profileTemp },
    },
  );
  await writeFile(resolve(output, 'lifecycle.stdout.txt'), lifecycle.stdout ?? '');
  await writeFile(resolve(output, 'lifecycle.stderr.txt'), lifecycle.stderr ?? '');
  if (lifecycle.error || lifecycle.status !== 0)
    throw new Error(`Packaged native runtime lifecycle failed: ${String(lifecycle.status)}`, {
      cause: lifecycle.error,
    });
  const after = await verifyHostedRuntimeTree(runtimeRoot, binding);
  const identity = Object.fromEntries(Object.entries(binding).filter(([key]) => key !== 'files'));
  const evidence = {
    ...identity,
    schemaVersion: 1,
    kind: 'github-hosted-native-runtime',
    result: 'passed',
    workflowRunId: process.env.GITHUB_RUN_ID,
    host: 'github-hosted',
    coverage: {
      installerPayload: 'verified',
      nativeRuntime: 'exercised',
      installation: 'not-exercised',
      uac: 'not-exercised',
    },
    runtimeFileCount: before.fileCount,
    runtimeVerifiedBefore: true,
    runtimeVerifiedAfter: after.fileCount === before.fileCount,
    lifecycle: JSON.parse(lifecycle.stdout),
  };
  validateHostedRuntimeEvidence(evidence, binding);
  await writeFile(
    resolve(output, `windows-hosted-runtime-${architecture}.json`),
    `${JSON.stringify(evidence, null, 2)}\n`,
  );
} catch (error) {
  await writeFile(
    resolve(output, 'failure.txt'),
    error instanceof Error ? (error.stack ?? error.message) : String(error),
  );
  throw error;
}
