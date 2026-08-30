import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { access, mkdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { basename, resolve } from 'node:path';
import { tmpdir } from 'node:os';

if (process.platform !== 'win32') throw new Error('Windows lifecycle requires Windows');
const architecture = valueAfter('--arch');
if (!['x64', 'arm64'].includes(architecture) || process.arch !== architecture) {
  throw new Error('Windows lifecycle requires an exact native --arch x64|arm64 host');
}
const mode = valueAfter('--mode') ?? 'unpacked';
const exerciseDurableDisconnect = process.argv.includes('--durable-disconnect');
if (!['unpacked', 'installed'].includes(mode))
  throw new Error('Expected --mode unpacked|installed');
const root = resolve(valueAfter('--root') ?? 'release/win-unpacked');
const application = resolve(root, 'Talking Quill.exe');
const helper = resolve(root, 'resources/helper/talking-quill-helper.exe');
const owner = resolve(root, 'resources/helper/talking-quill-keyboard-owner.exe');
const profile = resolve(
  tmpdir(),
  'TalkingQuillInstalledLifecycle',
  `${mode}-${process.pid.toString(10)}-${randomBytes(8).toString('hex')}`,
);
await mkdir(profile, { recursive: true });
// This unattended package check cannot safely synthesize a machine-global hotkey:
// SendInput would affect the interactive desktop and no package-scoped sender exists.
// It therefore proves authenticated owner startup and idle process drain, not an
// in-flight ownership transaction drain.
const ownershipCoverage = {
  status: 'unavailable',
  authoritative: false,
  exercise: 'idle-gateway-crash-and-sequential-relaunch',
  limitation:
    'No package-scoped physical input sender exists. This run does not exercise held-key ownership, terminal-incomplete behavior, or physical keyboard suppression.',
};
await Promise.all([access(application), access(helper), access(owner)]);
if (mode === 'installed') {
  const expectedInstalledRoot = resolve(String(process.env.ProgramFiles), 'Talking Quill');
  if (root.toLowerCase() !== expectedInstalledRoot.toLowerCase()) {
    throw new Error('Installed lifecycle root must be the exact Talking Quill Program Files root');
  }
}
await assertRolesAbsent();

const first = await readinessLaunch('initial');
const persistedSettings = JSON.parse(await readFile(resolve(profile, 'settings.json'), 'utf8'));
const persistedBindingCount = Array.isArray(persistedSettings.dictationProfiles)
  ? persistedSettings.dictationProfiles.length
  : 0;
const requiredApplicationChecks = [
  'talking-quill-application-running',
  'main-capture-widget-created',
  'persisted-profile-activation',
];
if (
  first.result !== 'passed' ||
  first.userDataRootSha256 !== createHash('sha256').update(profile).digest('hex') ||
  first.bindingCount !== persistedBindingCount ||
  persistedBindingCount === 0 ||
  !requiredApplicationChecks.every((check) => first.checks?.includes(check))
) {
  throw new Error(`Initial TalkingQuillApplication readiness failed: ${JSON.stringify(first)}`);
}

const durableDisconnect = exerciseDurableDisconnect ? await launchDurableDisconnect() : undefined;
if (durableDisconnect !== undefined) await waitForRolesAbsent(45_000);
const crash = await launchArmedLifecycle();
await waitForRolesAbsent(45_000);
const successor = await readinessLaunch('successor');
if (successor.result !== 'passed') {
  throw new Error(`Owner drain/relaunch readiness failed: ${JSON.stringify(successor)}`);
}
await waitForRolesAbsent(45_000);
console.log(
  JSON.stringify(
    {
      result: 'passed',
      architecture,
      mode,
      root,
      ownershipCoverage,
      first,
      ...(durableDisconnect === undefined ? {} : { durableDisconnect }),
      crash,
      successor,
    },
    null,
    2,
  ),
);

async function readinessLaunch(label) {
  const readiness = await oneUsePipe('InstalledReadiness');
  const correlation = randomBytes(32).toString('hex');
  const child = launch([
    `--talking-quill-installed-readiness-pipe=${readiness.name}`,
    `--talking-quill-launch-correlation=${correlation}`,
  ]);
  try {
    const [message, exitCode] = await Promise.all([
      within(readiness.message, 45_000, `${label} readiness timed out`),
      waitForExit(child, 45_000),
    ]);
    if (exitCode !== 0)
      throw new Error(
        `${label} application exited ${String(exitCode)}: ${JSON.stringify(message)}`,
      );
    return message;
  } finally {
    await readiness.close();
    if (child.exitCode === null) child.kill();
  }
}

async function launchDurableDisconnect() {
  const readiness = await oneUsePipe('InstalledReadiness');
  const armed = await oneUsePipe('AutomationArmed');
  const correlation = randomBytes(32).toString('hex');
  const child = launch([
    `--talking-quill-installed-readiness-pipe=${readiness.name}`,
    `--talking-quill-automation-armed-pipe=${armed.name}`,
    `--talking-quill-launch-correlation=${correlation}`,
    '--talking-quill-installed-automation-validation',
    '--talking-quill-automation-case=lifecycle',
  ]);
  try {
    const boundary = await within(armed.message, 45_000, 'Durable disconnect arm timed out');
    if (
      boundary?.phase !== 'armed' ||
      boundary?.correlation !== correlation ||
      boundary?.ownerIdentity?.authenticated !== true
    ) {
      throw new Error(`Invalid durable disconnect arm: ${JSON.stringify(boundary)}`);
    }
    const before = roleProcesses();
    if (before.gateways.length !== 1 || before.owners.length !== 1) {
      throw new Error(`Expected one gateway and owner: ${JSON.stringify(before)}`);
    }
    process.kill(before.owners[0].ProcessId);
    const checkpointPath = resolve(profile, 'logs', 'owner-connection-counts.json');
    const journalPath = resolve(profile, 'logs', 'owner-connection-helper-journal.json');
    const committed = await within(
      (async () => {
        for (;;) {
          try {
            const checkpoint = JSON.parse(await readFile(checkpointPath, 'utf8'));
            const journal = JSON.parse(await readFile(journalPath, 'utf8'));
            const entry = journal.entries?.[0];
            if (
              checkpoint.version === 2 &&
              checkpoint.acceptedDisconnects === '1' &&
              checkpoint.persistenceFailures === '0' &&
              checkpoint.journalCapacityRejects === '0' &&
              checkpoint.streamIdentityCollisions === '0' &&
              journal.durabilityFailures === '0' &&
              entry?.counter?.total === '1' &&
              entry?.counter?.acknowledged === '1'
            ) {
              return {
                oldOwnerPid: before.owners[0].ProcessId,
                gatewayPid: before.gateways[0].ProcessId,
                checkpointVersion: checkpoint.version,
                acceptedDisconnects: checkpoint.acceptedDisconnects,
                journalTotal: entry.counter.total,
                journalAcknowledged: entry.counter.acknowledged,
                durabilityFailures: journal.durabilityFailures,
                processGeneration: journal.processGeneration,
              };
            }
          } catch {
            // Atomic replacement can make the file briefly unavailable to a reader on Windows.
          }
          await delay(50);
        }
      })(),
      30_000,
      'Durable disconnect did not commit and acknowledge',
    );
    return committed;
  } finally {
    await Promise.all([readiness.close(), armed.close()]);
    if (child.exitCode === null) child.kill();
    await waitForExit(child, 10_000).catch(() => undefined);
  }
}

async function launchArmedLifecycle() {
  const readiness = await oneUsePipe('InstalledReadiness');
  const armed = await oneUsePipe('AutomationArmed');
  const correlation = randomBytes(32).toString('hex');
  const child = launch([
    `--talking-quill-installed-readiness-pipe=${readiness.name}`,
    `--talking-quill-automation-armed-pipe=${armed.name}`,
    `--talking-quill-launch-correlation=${correlation}`,
    '--talking-quill-installed-automation-validation',
    '--talking-quill-automation-case=lifecycle',
  ]);
  try {
    const boundary = await within(armed.message, 45_000, 'Lifecycle arm timed out');
    if (
      boundary?.phase !== 'armed' ||
      boundary?.correlation !== correlation ||
      boundary?.ownerIdentity?.authenticated !== true
    ) {
      throw new Error(`Invalid lifecycle arm: ${JSON.stringify(boundary)}`);
    }
    const before = roleProcesses();
    if (before.gateways.length !== 1 || before.owners.length !== 1) {
      throw new Error(`Expected one gateway and owner: ${JSON.stringify(before)}`);
    }
    process.kill(before.gateways[0].ProcessId);
    if (child.exitCode === null) child.kill();
    await waitForExit(child, 10_000).catch(() => undefined);
    return {
      gatewayPid: before.gateways[0].ProcessId,
      ownerPid: before.owners[0].ProcessId,
      ownerAuthenticated: true,
    };
  } finally {
    await Promise.all([readiness.close(), armed.close()]);
    if (child.exitCode === null) child.kill();
  }
}

function launch(arguments_) {
  return spawn(
    application,
    ['--disable-gpu', `--talking-quill-installed-lifecycle-user-data=${profile}`, ...arguments_],
    {
      stdio: 'ignore',
      windowsHide: true,
      env: process.env,
    },
  );
}

function roleProcesses() {
  const escapedRoot = root.replaceAll("'", "''");
  // The detached owner reports a Win32 device path (\\?\C:\...) while
  // Electron and the gateway report DOS paths. Normalize that prefix before
  // applying the exact installed-root boundary.
  const script = `$root='${escapedRoot}'.TrimEnd('\\');$prefix=$root+'\\';$items=@(Get-CimInstance Win32_Process|Where-Object{if(-not $_.ExecutablePath){return $false};$path=$_.ExecutablePath;if($path.StartsWith('\\\\?\\')){$path=$path.Substring(4)};$path.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)}|Select-Object ProcessId,ParentProcessId,Name,ExecutablePath);ConvertTo-Json -Compress -InputObject $items`;
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 15_000,
    },
  );
  if (result.status !== 0) throw new Error('Could not enumerate package processes');
  const parsed = JSON.parse(result.stdout.trim() || '[]');
  const values = Array.isArray(parsed) ? parsed : [parsed];
  return {
    gateways: values.filter(
      (item) => String(item.Name).toLowerCase() === basename(helper).toLowerCase(),
    ),
    owners: values.filter(
      (item) => String(item.Name).toLowerCase() === basename(owner).toLowerCase(),
    ),
    all: values,
  };
}

async function assertRolesAbsent() {
  const current = roleProcesses();
  if (current.all.length !== 0)
    throw new Error(
      `Lifecycle requires no stale package processes: ${JSON.stringify(current.all)}`,
    );
}
async function waitForRolesAbsent(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (roleProcesses().all.length === 0) return;
    await delay(100);
  }
  throw new Error(
    `Package processes remained after lifecycle: ${JSON.stringify(roleProcesses().all)}`,
  );
}

async function oneUsePipe(kind) {
  const name = `\\\\.\\pipe\\TalkingQuill.${kind}.${randomBytes(16).toString('hex')}`;
  let resolveMessage;
  let rejectMessage;
  const message = new Promise((resolveValue, rejectValue) => {
    resolveMessage = resolveValue;
    rejectMessage = rejectValue;
  });
  const server = createServer((socket) => {
    let body = '';
    socket.setEncoding('utf8');
    socket.on('data', (chunk) => (body += chunk));
    socket.once('error', rejectMessage);
    socket.once('end', () => {
      try {
        resolveMessage(JSON.parse(body.trim()));
      } catch (error) {
        rejectMessage(error);
      }
    });
  });
  server.once('error', rejectMessage);
  await new Promise((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(name, resolveListen);
  });
  return { name, message, close: () => new Promise((resolveClose) => server.close(resolveClose)) };
}
function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null) return Promise.resolve(child.exitCode);
  return within(
    new Promise((resolveExit, reject) => {
      child.once('exit', resolveExit);
      child.once('error', reject);
    }),
    timeoutMs,
    'Application exit timed out',
  );
}
async function within(operation, timeoutMs, message) {
  let timer;
  try {
    return await Promise.race([
      operation,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
function delay(ms) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms));
}
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
