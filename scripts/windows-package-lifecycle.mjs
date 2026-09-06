import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { access, mkdir, open, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { basename, isAbsolute, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { subprocessFailure } from './sanitized-subprocess-error.mjs';

export const DIAGNOSTIC_BYTE_LIMIT = 256 * 1024;

export function redactLifecycleDiagnostic(text, paths = []) {
  let result = String(text);
  for (const path of paths.filter(Boolean).sort((a, b) => b.length - a.length)) {
    result = result.replaceAll(JSON.stringify(path).slice(1, -1), '<path>');
    result = result.replaceAll(path, '<path>');
  }
  return result
    .replace(/(Bearer\s+)[^\s"']+/giu, '$1<redacted>')
    .replace(
      /((?:token|secret|password|api[-_]?key|correlation)\s*["']?\s*[:=]\s*["']?)[^\s"',}]+/giu,
      '$1<redacted>',
    )
    .replace(/\b[0-9a-f]{64}\b/giu, '<redacted>')
    .replace(/S-1-5-(?:\d+-)*\d+/gu, '<sid>');
}

export function boundedDiagnosticCapture(limit = DIAGNOSTIC_BYTE_LIMIT) {
  let bytes = 0;
  let kept = Buffer.alloc(0);
  return {
    append(chunk) {
      const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      bytes += buffer.length;
      kept = Buffer.concat([kept, buffer.subarray(0, Math.max(0, limit - kept.length))]);
    },
    snapshot() {
      return { text: kept.toString('utf8'), bytes, truncated: bytes > limit };
    },
  };
}

async function main() {
  const subprocessEnvironment = sanitizedSubprocessEnvironment();
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
  const diagnosticsInput = valueAfter('--diagnostics');
  const diagnostics = diagnosticsInput === undefined ? undefined : resolve(diagnosticsInput);
  if (diagnostics !== undefined) {
    const childPath = relative(resolve('tmp'), diagnostics);
    if (!childPath || childPath.startsWith('..') || isAbsolute(childPath))
      throw new Error('Lifecycle diagnostics must remain under project tmp');
    await mkdir(diagnostics, { recursive: true });
  }
  const launches = [];
  const profile = resolve(
    tmpdir(),
    'TalkingQuillInstalledLifecycle',
    `${mode}-${process.pid.toString(10)}-${randomBytes(8).toString('hex')}`,
  );
  await mkdir(profile, { recursive: true });
  const redact = (text) =>
    redactLifecycleDiagnostic(text, [
      profile,
      root,
      process.env.USERPROFILE,
      process.env.APPDATA,
      process.env.LOCALAPPDATA,
    ]);
  if (diagnostics !== undefined)
    await writeFile(
      resolve(diagnostics, 'launch-context.json'),
      JSON.stringify(
        {
          architecture,
          mode,
          application,
          root,
          profile,
          cwd: process.cwd(),
          osTemp: tmpdir(),
          windowsHide: true,
          requestedLifecycleProfile: profile,
          note: 'Requested profile is not proof that the production entry accepted the flag.',
        },
        null,
        2,
      ),
    );
  try {
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
        throw new Error(
          'Installed lifecycle root must be the exact Talking Quill Program Files root',
        );
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

    const durableDisconnect = exerciseDurableDisconnect
      ? await launchDurableDisconnect()
      : undefined;
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
  } catch (error) {
    if (diagnostics !== undefined)
      await writeFile(
        resolve(diagnostics, 'readiness-error.txt'),
        redact(error instanceof Error ? (error.stack ?? error.message) : error),
      );
    throw error;
  } finally {
    if (diagnostics !== undefined) {
      await collectDiagnostics().catch((error) => {
        console.error(`Lifecycle diagnostic collection failed: ${redact(String(error))}`);
      });
    }
  }

  async function readinessLaunch(label) {
    const readiness = await oneUsePipe('InstalledReadiness');
    const correlation = randomBytes(32).toString('hex');
    const child = launch(
      [
        `--talking-quill-installed-readiness-pipe=${readiness.name}`,
        `--talking-quill-launch-correlation=${correlation}`,
      ],
      label,
    );
    readiness.message.then(
      (message) => {
        child.lifecycleDiagnostic.readiness = message;
      },
      (error) => {
        child.lifecycleDiagnostic.readinessError = String(error);
      },
    );
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
      child.lifecycleDiagnostic.beforeCleanup = {
        exitCode: child.exitCode,
        signalCode: child.signalCode,
        alive: child.exitCode === null && child.signalCode === null,
      };
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

  function launch(arguments_, label = `launch-${launches.length + 1}`) {
    const child = spawn(
      application,
      ['--disable-gpu', `--talking-quill-installed-lifecycle-user-data=${profile}`, ...arguments_],
      {
        stdio: diagnostics === undefined ? 'ignore' : ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
        env: subprocessEnvironment,
      },
    );
    const stdout = boundedDiagnosticCapture();
    const stderr = boundedDiagnosticCapture();
    const record = {
      label,
      pid: child.pid,
      startedAt: new Date().toISOString(),
      stdout,
      stderr,
      child,
    };
    child.lifecycleDiagnostic = record;
    launches.push(record);
    child.stdout?.on('data', (chunk) => stdout.append(chunk));
    child.stderr?.on('data', (chunk) => stderr.append(chunk));
    child.once('error', (error) => {
      record.spawnError = String(error);
    });
    child.once('exit', (code, signal) => {
      record.exit = { code, signal, at: new Date().toISOString(), killRequested: child.killed };
    });
    return child;
  }

  async function collectDiagnostics() {
    // Give already-requested termination and pipe draining a bounded opportunity to finish.
    await Promise.all(
      launches.map(({ child }) =>
        child.exitCode !== null || child.signalCode !== null
          ? Promise.resolve()
          : waitForExit(child, 2_000).catch(() => undefined),
      ),
    );
    const records = [];
    for (const { child, stdout, stderr, ...record } of launches) {
      for (const [name, capture] of [
        ['stdout', stdout],
        ['stderr', stderr],
      ]) {
        const { text, ...metadata } = capture.snapshot();
        await writeFile(resolve(diagnostics, `${record.label}.${name}.txt`), redact(text));
        record[name] = metadata;
      }
      records.push({
        ...record,
        exitCode: child.exitCode,
        signalCode: child.signalCode,
        killRequested: child.killed,
      });
    }
    await writeFile(
      resolve(diagnostics, 'processes.json'),
      redact(JSON.stringify(records, null, 2)),
    );
    const sources = [['requested-profile', profile]];
    // The canonical entry may ignore acceptance-only profile flags. Do not read settings,
    // credentials, recordings or history from either profile, only known diagnostic files.
    if (process.env.APPDATA)
      sources.push(['default-profile', resolve(process.env.APPDATA, 'Talking Quill')]);
    const logs = [];
    for (const [label, directory] of sources) {
      for (const suffix of ['', '.1', '.2']) {
        const name = `diagnostic.jsonl${suffix}`;
        let handle;
        try {
          handle = await open(resolve(directory, 'logs', name), 'r');
          const stat = await handle.stat();
          if (!stat.isFile()) throw new Error('Not a regular diagnostic file');
          const buffer = Buffer.alloc(DIAGNOSTIC_BYTE_LIMIT);
          const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
          await writeFile(
            resolve(diagnostics, `${label}-${name}.txt`),
            redact(buffer.subarray(0, bytesRead).toString('utf8')),
          );
          logs.push({ label, name, bytes: stat.size, truncated: stat.size > bytesRead });
        } catch (error) {
          logs.push({ label, name, error: error.code ?? String(error) });
        } finally {
          await handle?.close();
        }
      }
    }
    await writeFile(resolve(diagnostics, 'app-logs.json'), redact(JSON.stringify(logs, null, 2)));
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
        // CIM startup on hosted Windows can exceed 15 seconds on a cold runner.
        timeout: 60_000,
        env: subprocessEnvironment,
      },
    );
    if (result.error !== undefined || result.signal !== null || result.status !== 0) {
      throw subprocessFailure('Package process enumeration', result);
    }
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
    const sockets = new Set();
    const server = createServer((socket) => {
      sockets.add(socket);
      socket.once('close', () => sockets.delete(socket));
      let body = '';
      socket.setEncoding('utf8');
      socket.on('data', (chunk) => {
        body += chunk;
        if (Buffer.byteLength(body) > DIAGNOSTIC_BYTE_LIMIT) {
          rejectMessage(new Error('Readiness pipe exceeded diagnostic byte limit'));
          socket.destroy();
        }
      });
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
    // Optional readiness pipes in armed launches may fail before anyone awaits them.
    void message.catch(() => undefined);
    return {
      name,
      message,
      close: () =>
        new Promise((resolveClose) => {
          for (const socket of sockets) socket.destroy();
          server.close(resolveClose);
        }),
    };
  }
  function waitForExit(child, timeoutMs) {
    if (child.exitCode !== null || child.signalCode !== null)
      return Promise.resolve(child.exitCode);
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
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await main();
}
