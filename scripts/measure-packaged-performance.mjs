import { execFile, execFileSync, spawn } from 'node:child_process';
import { mkdir, rm, stat, writeFile } from 'node:fs/promises';
import { cpus, version as osVersion } from 'node:os';
import { createServer } from 'node:net';
import { dirname, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { promisify } from 'node:util';
import { chromium } from '@playwright/test';
import {
  MIB,
  classifyWindowsProcess,
  describeBytes,
  range,
  summarizeProcesses,
} from './performance-metrics.mjs';

const execFileAsync = promisify(execFile);
const PRIVATE_WORKING_SET_BUDGET = 400 * MIB;
const STARTUP_BUDGET_MS = 3_000;
const options = readOptions(process.argv.slice(2));

if (process.platform !== 'win32') {
  throw new Error(
    'This measurement implementation currently requires Windows performance counters',
  );
}

const packageRoot = resolve(options.packageRoot);
const executable = resolve(packageRoot, 'Talking Quill.exe');
const packageInfo = await stat(executable).catch(() => null);
if (packageInfo === null) {
  throw new Error(`Packaged executable not found: ${executable}`);
}
const sourceCommit = execFileSync('git.exe', ['rev-parse', 'HEAD'], {
  encoding: 'utf8',
  timeout: 10_000,
}).trim();

const runs = [];
for (let run = 1; run <= options.runs; run += 1) {
  runs.push(await measureRun(run));
}

const startup = range(runs.map((run) => run.startupMs));
const privateWorkingSet = range(runs.map((run) => run.summary.privateWorkingSetBytes));
const grossWorkingSet = range(runs.map((run) => run.summary.grossWorkingSetBytes));
const privateBytes = range(runs.map((run) => run.summary.privateBytes));
const result = {
  contract: {
    startupBudgetMs: STARTUP_BUDGET_MS,
    idlePrivateWorkingSetBudgetBytes: PRIVATE_WORKING_SET_BUDGET,
    memoryMetric:
      'Sum of resident non-shared process pages (Win32 WorkingSetPrivate) across the owned process tree',
    launchCohort:
      'Fresh isolated profile; CDP is connected only through the Ready observation, then disconnected before the idle sample; no fake media or behavior/security override',
    diagnostics: {
      grossWorkingSet:
        'Sum of per-process WorkingSet64; analogous to aggregate RSS and double-counts shared pages',
      privateBytes:
        'Sum of private committed bytes; includes non-resident pages and is not the RAM budget metric',
    },
  },
  binding: {
    sourceCommit,
    packageRoot,
    packageExecutableModifiedAt: packageInfo.mtime.toISOString(),
    note: 'Build/package immediately before measurement; the unpacked directory has no cryptographic source marker.',
  },
  host: {
    platform: process.platform,
    architecture: process.arch,
    osVersion: osVersion(),
    logicalCpuCount: cpus().length,
  },
  results: {
    startupMs: startup,
    idlePrivateWorkingSetMiB: mapRange(privateWorkingSet, describeBytes),
    grossWorkingSetMiB: mapRange(grossWorkingSet, describeBytes),
    privateBytesMiB: mapRange(privateBytes, describeBytes),
    startupPassed: startup.max <= STARTUP_BUDGET_MS,
    idleMemoryPassed: privateWorkingSet.max <= PRIVATE_WORKING_SET_BUDGET,
  },
  runs: runs.map((run) => ({
    ...run,
    summary: formatSummary(run.summary),
    processes: run.processes.map(formatProcess),
  })),
};

const serialized = `${JSON.stringify(result, null, 2)}\n`;
process.stdout.write(serialized);
if (options.output !== null) {
  const output = resolve(options.output);
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, serialized, 'utf8');
}
if (!result.results.startupPassed || !result.results.idleMemoryPassed) process.exitCode = 1;

async function measureRun(run) {
  const marker = `talking-quill-performance-${String(process.pid)}-${String(Date.now())}-${String(run)}`;
  const runRoot = resolve('tmp', 'performance', marker);
  const appData = resolve(runRoot, 'appdata');
  const applicationProfile = resolve(appData, 'Talking Quill');
  await rm(runRoot, { recursive: true, force: true, maxRetries: 3 });
  await mkdir(applicationProfile, { recursive: true });

  const env = {
    ...Object.fromEntries(
      Object.entries(process.env).filter(([key]) => key.toLowerCase() !== 'appdata'),
    ),
    APPDATA: appData,
    CI: 'true',
    TALKING_QUILL_PACKAGED_TEST: '1',
  };
  let child = null;
  let browser = null;
  let diagnostics = '';
  const ownedPids = new Set();
  try {
    const startedAt = performance.now();
    const debuggingPort = await freePort();
    child = spawn(
      executable,
      [
        `--remote-debugging-port=${String(debuggingPort)}`,
        `--talking-quill-user-data=${applicationProfile}`,
      ],
      {
        env,
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      },
    );
    ownedPids.add(child.pid);
    for (const stream of [child.stdout, child.stderr]) {
      stream.on('data', (chunk) => {
        diagnostics = `${diagnostics}${chunk.toString()}`.slice(-8_192);
      });
    }
    const ready = await waitForReady(debuggingPort, startedAt).catch((error) => {
      throw new Error(
        `Packaged window did not become ready (exit=${String(child.exitCode)})\n${diagnostics}`,
        { cause: error },
      );
    });
    browser = ready.browser;
    const rendererRoles = ready.rendererRoles;
    await browser.close();
    browser = null;
    await delay(options.settleMs);
    const processes = await snapshotProcessTree(child.pid);
    for (const process of processes) ownedPids.add(process.pid);
    if (processes.length === 0) throw new Error('The packaged process tree disappeared');
    const summary = summarizeProcesses(processes);
    return { run, startupMs: ready.startupMs, rendererRoles, summary, processes };
  } finally {
    await browser?.close().catch(() => undefined);
    if (child !== null) await terminateOwnedTree(child.pid, ownedPids, marker);
  }
}

async function waitForReady(debuggingPort, startedAt) {
  const endpoint = `http://127.0.0.1:${String(debuggingPort)}`;
  const deadline = Date.now() + 15_000;
  let browser = null;
  while (Date.now() < deadline && browser === null) {
    browser = await chromium.connectOverCDP(endpoint, { timeout: 300 }).catch(() => null);
    if (browser === null) await delay(50);
  }
  if (browser === null) throw new Error('The debugging endpoint did not become ready');
  try {
    let main = null;
    while (Date.now() < deadline && main === null) {
      const pages = browser.contexts().flatMap((context) => context.pages());
      main = pages.find((page) => page.url().includes('/main/index.html')) ?? null;
      if (main === null) await delay(25);
    }
    if (main === null) throw new Error('The main renderer did not load');
    await main.locator('h1, h2').first().waitFor({ state: 'visible', timeout: 5_000 });
    const rendererRoles = browser
      .contexts()
      .flatMap((context) => context.pages())
      .map((page) => page.url().match(/\/(main|widget|capture)\/index\.html/)?.[1] ?? 'unknown')
      .sort();
    return { browser, rendererRoles, startupMs: Math.round(performance.now() - startedAt) };
  } catch (error) {
    await browser.close().catch(() => undefined);
    throw error;
  }
}

async function snapshotProcessTree(rootPid) {
  const script = `
$ErrorActionPreference='Stop'
$all=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine)
$ids=@([uint32]${String(rootPid)})
do { $before=$ids.Count; $ids=@($ids + @($all | Where-Object { $ids -contains $_.ParentProcessId } | ForEach-Object ProcessId) | Sort-Object -Unique) } while($ids.Count -gt $before)
$performance=@(Get-CimInstance Win32_PerfRawData_PerfProc_Process)
$rows=@()
foreach($item in @($all | Where-Object { $ids -contains $_.ProcessId })) {
  $process=Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue
  $counter=$performance | Where-Object { $_.IDProcess -eq $item.ProcessId } | Select-Object -First 1
  $rows += [pscustomobject]@{pid=[int]$item.ProcessId;ppid=[int]$item.ParentProcessId;name=[string]$item.Name;commandLine=[string]$item.CommandLine;privateWorkingSetBytes=$(if($null -eq $counter){$null}else{[int64]$counter.WorkingSetPrivate});grossWorkingSetBytes=$(if($null -eq $process){$null}else{[int64]$process.WorkingSet64});privateBytes=$(if($null -eq $process){$null}else{[int64]$process.PrivateMemorySize64})}
}
@($rows | Sort-Object pid) | ConvertTo-Json -Compress`;
  const output = (await runPowerShell(script, 20_000)).trim();
  if (output.length === 0) return [];
  const parsed = JSON.parse(output);
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  const incomplete = rows.filter(
    (row) =>
      row.privateWorkingSetBytes === null ||
      row.grossWorkingSetBytes === null ||
      row.privateBytes === null,
  );
  if (incomplete.length > 0) {
    throw new Error(
      `Performance counters were unavailable for owned PIDs: ${incomplete
        .map((row) => String(row.pid))
        .join(', ')}`,
    );
  }
  return rows.map((row) => ({
    ...row,
    role: classifyWindowsProcess(row.name, row.commandLine),
  }));
}

async function terminateOwnedTree(rootPid, ownedPids, marker) {
  for (const pid of await discoverOwnedPids(rootPid, marker)) ownedPids.add(pid);
  await execFileAsync('taskkill.exe', ['/PID', String(rootPid), '/T', '/F'], {
    timeout: 10_000,
    windowsHide: true,
  }).catch(() => undefined);
  for (const pid of ownedPids) {
    await execFileAsync('taskkill.exe', ['/PID', String(pid), '/F'], {
      timeout: 5_000,
      windowsHide: true,
    }).catch(() => undefined);
  }
  const ids = [...ownedPids];
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const remaining = [...new Set([...(await existingPids(ids)), ...(await markerPids(marker))])];
    if (remaining.length === 0) return;
    for (const pid of remaining) {
      await execFileAsync('taskkill.exe', ['/PID', String(pid), '/F'], {
        timeout: 5_000,
        windowsHide: true,
      }).catch(() => undefined);
    }
    await delay(100);
  }
  const remaining = [...new Set([...(await existingPids(ids)), ...(await markerPids(marker))])];
  throw new Error(`Packaged descendants survived cleanup: ${remaining.join(', ')}`);
}

async function discoverOwnedPids(rootPid, marker) {
  const escapedMarker = marker.replaceAll("'", "''");
  const script = `$all=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CommandLine); $ids=@([uint32]${String(
    rootPid,
  )}); do{$before=$ids.Count;$ids=@($ids + @($all | Where-Object { $ids -contains $_.ParentProcessId } | ForEach-Object ProcessId) | Sort-Object -Unique)}while($ids.Count -gt $before); @($all | Where-Object { $_.ProcessId -ne $PID -and ($ids -contains $_.ProcessId -or ([string]$_.CommandLine).Contains('${escapedMarker}')) } | ForEach-Object ProcessId | Sort-Object -Unique) | ConvertTo-Json -Compress`;
  const output = (await runPowerShell(script, 10_000)).trim();
  if (output.length === 0) return [];
  const parsed = JSON.parse(output);
  return Array.isArray(parsed) ? parsed : [parsed];
}

async function markerPids(marker) {
  const escapedMarker = marker.replaceAll("'", "''");
  const script = `@(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and ([string]$_.CommandLine).Contains('${escapedMarker}') } | ForEach-Object ProcessId) | ConvertTo-Json -Compress`;
  const output = (await runPowerShell(script, 10_000)).trim();
  if (output.length === 0) return [];
  const parsed = JSON.parse(output);
  return Array.isArray(parsed) ? parsed : [parsed];
}

async function existingPids(ids) {
  if (ids.length === 0) return [];
  const script = `$ids=@(${ids.join(',')}); @(Get-Process -Id $ids -ErrorAction SilentlyContinue | ForEach-Object Id) | ConvertTo-Json -Compress`;
  const output = (await runPowerShell(script, 10_000)).trim();
  if (output.length === 0) return [];
  const parsed = JSON.parse(output);
  return Array.isArray(parsed) ? parsed : [parsed];
}

async function runPowerShell(script, timeout) {
  const { stdout } = await execFileAsync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    { encoding: 'utf8', maxBuffer: 10 * MIB, timeout, windowsHide: true },
  );
  return stdout;
}

function formatSummary(summary) {
  return {
    processCount: summary.processCount,
    privateWorkingSetMiB: describeBytes(summary.privateWorkingSetBytes),
    grossWorkingSetMiB: describeBytes(summary.grossWorkingSetBytes),
    privateBytesMiB: describeBytes(summary.privateBytes),
    roles: Object.fromEntries(
      Object.entries(summary.roles).map(([role, value]) => [
        role,
        {
          count: value.count,
          privateWorkingSetMiB: describeBytes(value.privateWorkingSetBytes),
          grossWorkingSetMiB: describeBytes(value.grossWorkingSetBytes),
          privateBytesMiB: describeBytes(value.privateBytes),
        },
      ]),
    ),
  };
}

function formatProcess(process) {
  return {
    pid: process.pid,
    ppid: process.ppid,
    name: process.name,
    role: process.role,
    privateWorkingSetMiB: describeBytes(process.privateWorkingSetBytes),
    grossWorkingSetMiB: describeBytes(process.grossWorkingSetBytes),
    privateBytesMiB: describeBytes(process.privateBytes),
    commandLine: process.commandLine,
  };
}

function mapRange(value, mapper) {
  return { min: mapper(value.min), median: mapper(value.median), max: mapper(value.max) };
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (typeof address !== 'object' || address === null) {
        server.close();
        reject(new Error('Unable to allocate a debugging port'));
        return;
      }
      server.close(() => resolvePort(address.port));
    });
  });
}

function readOptions(arguments_) {
  const values = { packageRoot: 'release/win-unpacked', runs: 3, settleMs: 5_000, output: null };
  for (const argument of arguments_) {
    if (argument === '--') continue;
    if (argument.startsWith('--package-root=')) values.packageRoot = argument.slice(15);
    else if (argument.startsWith('--runs=')) values.runs = Number(argument.slice(7));
    else if (argument.startsWith('--settle-ms=')) values.settleMs = Number(argument.slice(12));
    else if (argument.startsWith('--output=')) values.output = argument.slice(9);
    else throw new Error(`Unknown argument: ${argument}`);
  }
  if (!Number.isInteger(values.runs) || values.runs < 3)
    throw new Error('--runs must be at least 3');
  if (!Number.isInteger(values.settleMs) || values.settleMs < 1_000) {
    throw new Error('--settle-ms must be at least 1000');
  }
  return values;
}
