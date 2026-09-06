import { spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { redactLifecycleDiagnostic } from './windows-package-lifecycle.mjs';
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
  const diagnostics = resolve(output, 'diagnostics');
  const observer = spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      'scripts/windows-production-startup-smoke.ps1',
      '-Architecture',
      architecture,
      '-RuntimeRoot',
      resolve(runtimeRoot),
      '-OutputDirectory',
      diagnostics,
    ],
    {
      encoding: 'utf8',
      timeout: 180_000,
      maxBuffer: 1024 * 1024,
      env: sanitizedSubprocessEnvironment(process.env, {
        GITHUB_ACTIONS: 'true',
        RUNNER_ENVIRONMENT: 'github-hosted',
        RUNNER_OS: 'Windows',
        GITHUB_RUN_ID: process.env.GITHUB_RUN_ID,
      }),
    },
  );
  const redact = (text) =>
    redactLifecycleDiagnostic(text, [
      process.env.USERPROFILE,
      process.env.APPDATA,
      process.env.LOCALAPPDATA,
    ]);
  await writeFile(
    resolve(output, 'observer-process.json'),
    JSON.stringify(
      {
        status: observer.status,
        signal: observer.signal,
        pid: observer.pid,
        error: observer.error
          ? { code: observer.error.code, message: redact(observer.error.message) }
          : null,
      },
      null,
      2,
    ),
  );
  await writeFile(resolve(output, 'observer.stdout.txt'), redact(observer.stdout ?? ''));
  await writeFile(resolve(output, 'observer.stderr.txt'), redact(observer.stderr ?? ''));
  // Verify the tree after both successful and failed startup attempts.
  const after = await verifyHostedRuntimeTree(runtimeRoot, binding);
  if (observer.error || observer.status !== 0)
    throw new Error(`Packaged production startup observer failed: ${String(observer.status)}`, {
      cause: observer.error,
    });
  const startup = JSON.parse(await readFile(resolve(diagnostics, 'startup-report.json'), 'utf8'));
  const screenshot = await readFile(resolve(diagnostics, 'startup-window.png'));
  if (!screenshot.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])))
    throw new Error('Production startup screenshot is missing or not PNG');
  const identity = Object.fromEntries(Object.entries(binding).filter(([key]) => key !== 'files'));
  const evidence = {
    ...identity,
    schemaVersion: 2,
    kind: 'github-hosted-native-runtime',
    result: 'passed',
    workflowRunId: process.env.GITHUB_RUN_ID,
    host: 'github-hosted',
    coverage: {
      installerPayload: 'verified',
      nativeRuntime: 'startup-observed',
      ownerAuthentication: 'not-observed',
      transactions: 'not-exercised',
      gracefulLifecycle: 'not-asserted',
      installation: 'not-exercised',
      uac: 'not-exercised',
    },
    runtimeFileCount: before.fileCount,
    runtimeVerifiedBefore: true,
    runtimeVerifiedAfter: after.fileCount === before.fileCount,
    startup,
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
