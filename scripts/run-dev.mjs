import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');

const forwardedArguments = process.argv.slice(2);
if (forwardedArguments[0] === '--') forwardedArguments.shift();
const helpOnly = forwardedArguments.length === 1 && forwardedArguments[0] === '--help';

let activeChild = null;
let requestedSignal = null;
const signalHandlers = new Map(
  ['SIGINT', 'SIGTERM'].map((signal) => {
    const handler = () => {
      requestedSignal ??= signal;
      if (activeChild?.exitCode === null && activeChild.signalCode === null) {
        activeChild.kill(signal);
      }
    };
    process.once(signal, handler);
    return [signal, handler];
  }),
);

let failure = null;
let restoreFailure = null;
let nativeRebuildStarted = false;
try {
  if (helpOnly) {
    await runPnpm([
      '--filter',
      '@talking-quill/app',
      'exec',
      'electron-vite',
      'dev',
      ...forwardedArguments,
    ]);
  } else {
    await runNode('scripts/build-helper.mjs');
    await runNode('scripts/build-preloads.mjs');
    await runNode('scripts/build-whisper-worker.mjs');
    nativeRebuildStarted = true;
    await runPnpm(['--filter', '@talking-quill/app', 'rebuild:native']);
    await runPnpm([
      '--filter',
      '@talking-quill/app',
      'exec',
      'electron-vite',
      'dev',
      ...forwardedArguments,
    ]);
  }
} catch (error) {
  failure = error;
} finally {
  if (nativeRebuildStarted) {
    try {
      await runNode('scripts/rebuild-node-native.mjs', { allowAfterSignal: true });
    } catch (error) {
      restoreFailure = error;
    }
  }
  for (const [signal, handler] of signalHandlers) process.removeListener(signal, handler);
}

if (restoreFailure !== null) {
  if (failure === null) throw restoreFailure;
  throw new AggregateError(
    [failure, restoreFailure],
    'Development startup failed and the host Node.js native module ABI could not be restored',
  );
}
if (requestedSignal !== null) process.exitCode = requestedSignal === 'SIGINT' ? 130 : 143;
else if (failure !== null) throw failure;

async function runNode(script, options) {
  await run(process.execPath, [resolve(repositoryRoot, script)], script, options);
}

async function runPnpm(args) {
  await run(process.execPath, [pnpmCli, ...args], `pnpm ${args.join(' ')}`);
}

async function run(command, args, label, { allowAfterSignal = false } = {}) {
  if (requestedSignal !== null && !allowAfterSignal) throw new Error('Development run interrupted');
  const child = spawn(command, args, { cwd: repositoryRoot, stdio: 'inherit' });
  activeChild = child;
  const result = await new Promise((resolveExit, reject) => {
    child.once('error', reject);
    child.once('exit', (code, signal) => resolveExit({ code, signal }));
  });
  if (activeChild === child) activeChild = null;
  if (result.code !== 0) {
    throw new Error(
      result.signal === null
        ? `${label} exited with code ${String(result.code)}`
        : `${label} exited due to ${result.signal}`,
    );
  }
  if (requestedSignal !== null && !allowAfterSignal) throw new Error('Development run interrupted');
}
