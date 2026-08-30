import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

import {
  checkProtectedBootstrapInclude,
  protectedBootstrapSourcePath,
  renderProtectedBootstrapInclude,
} from './windows-protected-bootstrap-generator.mjs';

const repositoryRoot = resolve(import.meta.dirname, '..');
const temporaryRoot = resolve(repositoryRoot, 'tmp');
const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const appBuilderRoot = dirname(electronBuilderRequire.resolve('app-builder-lib/package.json'));
const { getMakeNsisPath } = electronBuilderRequire(
  join(appBuilderRoot, 'out', 'toolsets', 'windows.js'),
);

const installerInclude = resolve(repositoryRoot, 'build', 'installer.nsh');
const makeNsisArguments = ['-WX', '-V2', '-NOCD'];
let windowsJobSupervisor;
const [source, bootstrapPayloadInclude, bootstrapSource] = await Promise.all([
  readFile(installerInclude, 'utf8'),
  checkProtectedBootstrapInclude(),
  readFile(resolve(repositoryRoot, 'build', 'windows-protected-bootstrap.ps1'), 'utf8'),
]);
const bootstrapPayload = requireBootstrapPayload(bootstrapPayloadInclude);
const protectedMacro = source.slice(
  source.indexOf('!macro TalkingQuillProtectedEarlyBootstrap'),
  source.indexOf('!macroend', source.indexOf('!macro TalkingQuillProtectedEarlyBootstrap')),
);
if (
  (protectedMacro.match(/ExecShellWait "open" "\$SYSDIR\\WindowsPowerShell/gu) ?? []).length !==
    2 ||
  (protectedMacro.match(/-WindowStyle Hidden/gu) ?? []).length !== 2 ||
  (protectedMacro.match(/SW_HIDE \$R2/gu) ?? []).length !== 2 ||
  /\bExecWait\b/u.test(protectedMacro)
) {
  throw new Error(
    'Protected PowerShell bootstrap must use two plugin-free hidden ExecShellWait calls',
  );
}
if (
  !source.includes('/TQOUTERWINDOW=$HWNDPARENT') ||
  !source.includes('.talking-quill-bootstrap-validated') ||
  !bootstrapSource.includes('TerminateWaitingInstallerParents') ||
  !bootstrapSource.includes("'.talking-quill-bootstrap-validated'")
) {
  throw new Error(
    'Protected ShellExecute bootstrap must propagate exact exit codes and authenticate validation completion',
  );
}
const abortFunction = source.slice(
  source.indexOf('Function TalkingQuillOnUserAbort'),
  source.indexOf('FunctionEnd', source.indexOf('Function TalkingQuillOnUserAbort')),
);
if (
  !source.includes('!define MUI_CUSTOMFUNCTION_ABORT TalkingQuillOnUserAbort') ||
  !abortFunction.includes('SetErrorLevel 0')
) {
  throw new Error('Graceful interactive NSIS cancellation must return exact exit code 0');
}
if (/-Command[^\r\n]*"\s+"\$[R0-9]/u.test(source)) {
  throw new Error(
    'Protected bootstrap must not append runtime arguments after PowerShell -Command',
  );
}

if (source.includes('StrCpy $TEMP')) {
  throw new Error('NSIS shell variable $TEMP must not be used as a StrCpy destination');
}
if (!source.includes('${If} $TEMP != $R1')) {
  throw new Error(
    'protected bootstrap must reject an NSIS TEMP that differs from the validated path',
  );
}

await mkdir(temporaryRoot, { recursive: true });
const workDirectory = await mkdtemp(join(temporaryRoot, 'nsis-early-init-'));
try {
  const uninstallerRoot = join(workDirectory, 'Program Files fixture root');
  await mkdir(uninstallerRoot);
  const fixtureProjectDirectory = await writeFixtureBootstrapInclude(
    workDirectory,
    uninstallerRoot,
  );
  const makensis = await getMakeNsisPath(undefined);
  for (const mode of ['installer', 'uninstaller']) {
    const scriptPath = join(workDirectory, `${mode}.nsi`);
    const outputPath = join(workDirectory, `${mode}-macro-check.exe`);
    await writeFile(
      scriptPath,
      macroCompileScript({ mode, outputPath, projectDirectory: fixtureProjectDirectory }),
      'utf8',
    );

    const result = spawnSync(makensis.path, [...makeNsisArguments, scriptPath], {
      cwd: appBuilderRoot,
      encoding: 'utf8',
      env: { ...process.env, ...(makensis.env ?? {}) },
      timeout: 60_000,
      windowsHide: true,
    });
    if (result.error || result.status !== 0) {
      const detail = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
      throw new Error(
        `${mode} early-init macro compile failed${result.error ? `: ${result.error.message}` : ''}${detail ? `\n${detail}` : ''}`,
      );
    }
    await requireGuiSubsystem(outputPath, `${mode} NSIS outer`);
  }
  const cancellationInstaller = join(workDirectory, 'nsis-cancellation-exit-check.exe');
  const cancellationScript = join(workDirectory, 'nsis-cancellation-exit-check.nsi');
  const cancellationLicense = join(workDirectory, 'nsis-cancellation-license.txt');
  await Promise.all([
    writeFile(cancellationLicense, 'Talking Quill cancellation runtime fixture.\n', 'utf8'),
    writeFile(
      cancellationScript,
      cancellationCompileScript({
        outputPath: cancellationInstaller,
        licensePath: cancellationLicense,
        projectDirectory: fixtureProjectDirectory,
      }),
      'utf8',
    ),
  ]);
  requireSuccess(
    spawnSync(
      makensis.path,
      [...makeNsisArguments.filter((argument) => argument !== '-WX'), cancellationScript],
      {
        cwd: appBuilderRoot,
        encoding: 'utf8',
        env: { ...process.env, ...(makensis.env ?? {}) },
        timeout: 60_000,
        windowsHide: true,
      },
    ),
    'NSIS interactive cancellation fixture compilation',
  );
  await requireGuiSubsystem(cancellationInstaller, 'NSIS interactive cancellation fixture');
  if (process.platform === 'win32') {
    windowsJobSupervisor = await buildWindowsJobSupervisor(workDirectory);
    await testWindowsJobSupervisor(workDirectory);
    await testNsisCancellationExit(workDirectory, cancellationInstaller);
    await testStaleCleanupRefusals(workDirectory);
    const startingResidue = await snapshotProtectedBootstrapResidue();
    console.log(
      `Protected-bootstrap pre-existing matching residue: ${String(startingResidue.names.length)}`,
    );
    try {
      for (let cycle = 1; cycle <= 3; cycle++) {
        await runCompiledNsisRuntimeHarness(workDirectory, uninstallerRoot);
        await assertProtectedBootstrapResidueUnchanged(
          startingResidue,
          `compiled protected bootstrap runtime harness cycle ${String(cycle)}`,
        );
        await runProtectedBootstrapIntegration(workDirectory, bootstrapPayload);
        await assertProtectedBootstrapResidueUnchanged(
          startingResidue,
          `complete protected bootstrap runtime harness cycle ${String(cycle)}`,
        );
      }
    } finally {
      await assertProtectedBootstrapResidueUnchanged(
        startingResidue,
        'complete protected bootstrap runtime harness',
      );
    }
  }
} finally {
  await rm(workDirectory, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
}

async function requireGuiSubsystem(path, label) {
  const bytes = await readFile(path);
  if (bytes.length < 96 || bytes.readUInt16LE(0) !== 0x5a4d) {
    throw new Error(`${label} is not a PE executable`);
  }
  const pe = bytes.readUInt32LE(0x3c);
  const subsystemOffset = pe + 24 + 68;
  if (
    subsystemOffset + 2 > bytes.length ||
    bytes.readUInt32LE(pe) !== 0x0000_4550 ||
    bytes.readUInt16LE(subsystemOffset) !== 2
  ) {
    throw new Error(`${label} must retain the Windows GUI subsystem`);
  }
}

function cancellationCompileScript({ outputPath, licensePath, projectDirectory }) {
  return `Unicode true
Name "Talking Quill cancellation exit check"
OutFile "${nsisPath(outputPath)}"
RequestExecutionLevel user
!define APP_GUID "early-init-check"
!define UNINSTALL_APP_KEY "early-init-check"
!define APP_FILENAME "Talking Quill"
!define PROJECT_DIR "${nsisPath(projectDirectory)}"
!define TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD
!include "MUI2.nsh"
!include "${nsisPath(installerInclude)}"
!insertmacro MUI_PAGE_LICENSE "${nsisPath(licensePath)}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Section
  SetErrorLevel 91
SectionEnd
Function .onInit
  !insertmacro customInit
FunctionEnd
`;
}

async function testNsisCancellationExit(workDirectory, installer) {
  const sourcePath = join(workDirectory, 'NsisCancellationController.cs');
  const controller = join(workDirectory, 'nsis-cancellation-controller.exe');
  await writeFile(sourcePath, nsisCancellationControllerSource(), 'utf8');
  requireSuccess(
    spawnSync(windowsCSharpCompiler(), ['/nologo', `/out:${controller}`, sourcePath], {
      encoding: 'utf8',
      timeout: 60_000,
      windowsHide: true,
    }),
    'NSIS cancellation controller compilation',
  );
  requireStatus(
    spawnSupervisedWithArguments(controller, [installer], process.env, 35_000),
    0,
    'compiled NSIS graceful interactive cancellation',
  );
}

function nsisCancellationControllerSource() {
  return `using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

internal static class NsisCancellationController
{
    private delegate bool EnumWindowsCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder text, int maximum);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int maximum);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll")] private static extern IntPtr GetDlgItem(IntPtr window, int controlId);
    [DllImport("user32.dll", SetLastError = true)] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    private const uint WmCommand = 0x0111;
    private const int IdCancel = 2;
    private const int IdYes = 6;

    public static int Main(string[] args)
    {
        if (args.Length != 1) return 126;
        string installer = Path.GetFullPath(args[0]);
        string programData = Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData);
        string[] baselineLeaves = ProtectedLeaves(programData);
        var knownWindows = new HashSet<long>();
        Process outer = null;
        IntPtr target = IntPtr.Zero, confirmation = IntPtr.Zero;
        bool cancelAccepted = false, confirmationAccepted = false;
        try
        {
            outer = Process.Start(new ProcessStartInfo(installer) { UseShellExecute = false });
            Log("process role=compiled-callback pid=" + outer.Id + " image=" + installer);
            DateTime startupDeadline = DateTime.UtcNow.AddSeconds(15);
            while (DateTime.UtcNow < startupDeadline && target == IntPtr.Zero)
            {
                foreach (IntPtr window in Windows())
                {
                    uint pid; GetWindowThreadProcessId(window, out pid);
                    string cls = WindowClass(window);
                    if (knownWindows.Add(window.ToInt64()))
                        Log("window hwnd=" + window.ToInt64() + " pid=" + pid + " class=" + cls + " title=" + WindowTitle(window));
                    IntPtr cancel = GetDlgItem(window, IdCancel);
                    if (pid == outer.Id && cls == "#32770" && cancel != IntPtr.Zero &&
                        IsWindowVisible(cancel) && IsWindowEnabled(cancel)) target = window;
                }
                if (target == IntPtr.Zero) Thread.Sleep(20);
            }
            if (target == IntPtr.Zero)
                return Fail(124, "compiled callback NSIS UI window was not observed");
            uint targetPid; GetWindowThreadProcessId(target, out targetPid);
            if (targetPid != outer.Id)
                return Fail(124, "cancellation target was not owned by the compiled callback fixture");
            IntPtr cancelButton = GetDlgItem(target, IdCancel);
            Log("event=cancel-ready hwnd=" + target.ToInt64() + " button=" + cancelButton.ToInt64() +
                " visible=" + IsWindowVisible(cancelButton) + " enabled=" + IsWindowEnabled(cancelButton));
            cancelAccepted = PostMessage(target, WmCommand, new IntPtr(IdCancel), IntPtr.Zero);
            Log("event=IDCANCEL hwnd=" + target.ToInt64() + " pid=" + targetPid + " accepted=" + cancelAccepted);
            if (!cancelAccepted) return Fail(124, "IDCANCEL was not accepted");

            DateTime exitDeadline = DateTime.UtcNow.AddSeconds(10);
            DateTime nextCancelAttempt = DateTime.UtcNow.AddMilliseconds(50);
            while (DateTime.UtcNow < exitDeadline)
            {
                if (confirmation == IntPtr.Zero && !outer.HasExited && DateTime.UtcNow >= nextCancelAttempt)
                {
                    uint currentPid; GetWindowThreadProcessId(target, out currentPid);
                    if (currentPid != outer.Id || !PostMessage(target, WmCommand, new IntPtr(IdCancel), IntPtr.Zero))
                        return Fail(124, "IDCANCEL retry target changed or rejected delivery");
                    Log("event=IDCANCEL-retry hwnd=" + target.ToInt64() + " pid=" + currentPid);
                    nextCancelAttempt = DateTime.UtcNow.AddMilliseconds(50);
                }
                foreach (IntPtr window in Windows())
                {
                    uint pid; GetWindowThreadProcessId(window, out pid);
                    string cls = WindowClass(window);
                    if (knownWindows.Add(window.ToInt64()))
                        Log("window hwnd=" + window.ToInt64() + " pid=" + pid + " class=" + cls + " title=" + WindowTitle(window));
                    if (pid == outer.Id && window != target && cls == "#32770")
                    {
                        if (confirmation == IntPtr.Zero)
                        {
                            confirmation = window;
                            Log("event=cancel-confirmation hwnd=" + window.ToInt64() + " pid=" + pid);
                        }
                        confirmationAccepted |= PostMessage(window, WmCommand, new IntPtr(IdYes), IntPtr.Zero);
                    }
                }
                if (ExitedWith(outer, 0, "compiled callback fixture") &&
                    baselineLeaves.SequenceEqual(ProtectedLeaves(programData), StringComparer.OrdinalIgnoreCase))
                {
                    Log("result=pass callback-exit=0 descendants=0 protected-residue=baseline");
                    return 0;
                }
                Thread.Sleep(20);
            }
            return Fail(124, "bounded cancellation teardown did not complete");
        }
        catch (Exception error) { return Fail(125, error.ToString()); }
        finally
        {
            if (outer != null && !outer.HasExited)
            {
                outer.Kill();
                if (!outer.WaitForExit(5000)) Log("cleanup=outer-still-alive");
            }
            if (outer != null) outer.Dispose();
        }
    }

    private static bool ExitedWith(Process process, int expected, string role)
    {
        if (process == null || !process.HasExited) return false;
        if (process.ExitCode != expected) throw new InvalidOperationException(role + " exited " + process.ExitCode);
        return true;
    }

    private static List<IntPtr> Windows()
    {
        var windows = new List<IntPtr>();
        EnumWindows(delegate(IntPtr window, IntPtr data) { if (IsWindowVisible(window)) windows.Add(window); return true; }, IntPtr.Zero);
        return windows;
    }

    private static string WindowClass(IntPtr window) { var text = new StringBuilder(256); GetClassName(window, text, text.Capacity); return text.ToString(); }
    private static string WindowTitle(IntPtr window) { var text = new StringBuilder(1024); GetWindowText(window, text, text.Capacity); return text.ToString(); }
    private static string[] ProtectedLeaves(string programData) { return Directory.GetDirectories(programData, ".Talking Quill.Installer-*", SearchOption.TopDirectoryOnly).OrderBy(value => value, StringComparer.OrdinalIgnoreCase).ToArray(); }
    private static void Log(string message) { Console.WriteLine(DateTime.UtcNow.ToString("o") + " " + message); }
    private static int Fail(int code, string message) { Console.Error.WriteLine(DateTime.UtcNow.ToString("o") + " " + message); return code; }
}`;
}

async function buildWindowsJobSupervisor(workDirectory) {
  const compiler = windowsCSharpCompiler();
  const supervisor = join(workDirectory, 'windows-job-object-supervisor.exe');
  requireSuccess(
    spawnSync(
      compiler,
      [
        '/nologo',
        `/out:${supervisor}`,
        resolve(repositoryRoot, 'scripts', 'windows-job-object-supervisor.cs'),
      ],
      { encoding: 'utf8', timeout: 60_000, windowsHide: true },
    ),
    'Windows Job Object supervisor compilation',
  );
  return supervisor;
}

async function testWindowsJobSupervisor(workDirectory) {
  const sourcePath = join(workDirectory, 'JobSupervisorFixture.cs');
  const executable = join(workDirectory, 'job-supervisor-fixture.exe');
  await writeFile(sourcePath, jobSupervisorFixtureSource(), 'utf8');
  requireSuccess(
    spawnSync(windowsCSharpCompiler(), ['/nologo', `/out:${executable}`, sourcePath], {
      encoding: 'utf8',
      timeout: 60_000,
      windowsHide: true,
    }),
    'Job Object adversarial fixture compilation',
  );

  for (const [mode, timeout, expected] of [
    ['parent-exits', 10_000, 23],
    ['timeout', 500, 124],
  ]) {
    const pidPath = join(workDirectory, `job-${mode}.pid`);
    const result = spawnSupervisedWithArguments(executable, [mode, pidPath], process.env, timeout);
    requireStatus(result, expected, `Job Object ${mode} descendant containment`);
    const descendantPid = Number.parseInt((await readFile(pidPath, 'utf8')).trim(), 10);
    if (!Number.isSafeInteger(descendantPid) || descendantPid < 1) {
      throw new Error(`Job Object ${mode} fixture wrote an invalid descendant PID`);
    }
    const probe = spawnSync(
      resolve(
        process.env.WINDIR ?? String.raw`C:\Windows`,
        'System32',
        'WindowsPowerShell',
        'v1.0',
        'powershell.exe',
      ),
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `if (Get-Process -Id ${String(descendantPid)} -ErrorAction SilentlyContinue) { exit 1 }`,
      ],
      { encoding: 'utf8', timeout: 30_000, windowsHide: true },
    );
    requireSuccess(probe, `Job Object ${mode} descendant termination confirmation`);
  }
}

async function testStaleCleanupRefusals(workDirectory) {
  for (const [label, unknownContent, expectedStatus] of [
    ['recent', false, 'recent'],
    ['unknown-content', true, 'refused'],
  ]) {
    const runId = randomHarnessSuffix();
    const path = createProtectedHarnessLeaf(runId, unknownContent, false);
    try {
      const result = spawnSupervisedWithArguments(nativePowerShellPath(), [
        '-NoProfile',
        '-NonInteractive',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        resolve(repositoryRoot, 'scripts', 'cleanup-windows-protected-bootstrap-leaves.ps1'),
        '-RunId',
        runId,
      ]);
      requireSuccess(result, `stale cleanup ${label} refusal`);
      if (!new RegExp(`\\b${expectedStatus}\\b`, 'u').test(result.stdout)) {
        throw new Error(
          `stale cleanup ${label} fixture was not refused as ${expectedStatus}: ${result.stdout}`,
        );
      }
    } finally {
      removeProtectedHarnessLeaf(path);
    }
  }

  const liveRunId = randomHarnessSuffix();
  const livePath = createProtectedHarnessLeaf(liveRunId, false, true);
  const marker = join(workDirectory, 'cleanup-live-environment.ready');
  const sleeper = startSupervisedEnvironmentSleeper(livePath, marker);
  try {
    try {
      await waitForFile(marker, 10_000);
      const result = spawnSupervisedWithArguments(nativePowerShellPath(), [
        '-NoProfile',
        '-NonInteractive',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        resolve(repositoryRoot, 'scripts', 'cleanup-windows-protected-bootstrap-leaves.ps1'),
        '-RunId',
        liveRunId,
      ]);
      requireSuccess(result, 'stale cleanup live-environment refusal');
      if (!/\blive\b/u.test(result.stdout)) {
        throw new Error(`stale cleanup missed a live environment reference: ${result.stdout}`);
      }
    } finally {
      if (sleeper.exitCode === null && sleeper.signalCode === null) {
        sleeper.kill();
        await new Promise((resolveExit) => sleeper.once('exit', resolveExit));
      }
    }
    const removal = spawnSupervisedWithArguments(nativePowerShellPath(), [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      resolve(repositoryRoot, 'scripts', 'cleanup-windows-protected-bootstrap-leaves.ps1'),
      '-Apply',
      '-RunId',
      liveRunId,
    ]);
    if (removal.status !== 0 || !/\bremoved\b/u.test(removal.stdout)) {
      throw new Error(`stale cleanup did not remove its dead test leaf: ${removal.stdout}`);
    }
  } finally {
    removeProtectedHarnessLeaf(livePath);
  }
}

function startSupervisedEnvironmentSleeper(leaf, marker) {
  const command = `[IO.File]::WriteAllText('${marker.replaceAll("'", "''")}',[string]$PID);Start-Sleep -Seconds 300`;
  const rawArguments = ['-NoProfile', '-NonInteractive', '-Command', command]
    .map(quoteWindowsArgument)
    .join(' ');
  return spawn(
    windowsJobSupervisor,
    ['300000', nativePowerShellPath(), Buffer.from(rawArguments, 'utf8').toString('base64')],
    {
      env: { ...process.env, TQ_CLEANUP_LIVE_LEAF: leaf },
      stdio: 'ignore',
      windowsHide: true,
    },
  );
}

async function waitForFile(path, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      await readFile(path);
      return;
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
  }
  throw new Error(`Timed out waiting for fixture file: ${path}`);
}

function createProtectedHarnessLeaf(runId, unknownContent, old) {
  const command = String.raw`$pd=[Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData).TrimEnd('\');$p=Join-Path $pd '.Talking Quill.Harness-${runId}';$a=New-Object Security.AccessControl.DirectorySecurity;$a.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')));$a.SetAccessRuleProtection($true,$false);foreach($s in @('S-1-5-18','S-1-5-32-544')){$i=New-Object Security.Principal.SecurityIdentifier($s);$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($i,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))};[IO.Directory]::CreateDirectory($p,$a)|Out-Null;${unknownContent ? "[IO.File]::WriteAllText((Join-Path $p 'unknown.bin'),'x');" : ''}${old ? '$d=[DateTime]::UtcNow.AddDays(-2);[IO.Directory]::SetCreationTimeUtc($p,$d);[IO.Directory]::SetLastWriteTimeUtc($p,$d);' : ''}[Console]::Write($p)`;
  const result = spawnSync(
    nativePowerShellPath(),
    ['-NoProfile', '-NonInteractive', '-Command', command],
    {
      encoding: 'utf8',
      timeout: 30_000,
      windowsHide: true,
    },
  );
  requireSuccess(result, 'protected stale-cleanup fixture creation');
  return result.stdout.trim();
}

function removeProtectedHarnessLeaf(path) {
  const escaped = path.replaceAll("'", "''");
  const command = `for($i=0;$i -lt 20 -and (Test-Path -LiteralPath '${escaped}');$i++){Remove-Item -LiteralPath '${escaped}' -Recurse -Force -ErrorAction SilentlyContinue;if(Test-Path -LiteralPath '${escaped}'){Start-Sleep -Milliseconds 250}};if(Test-Path -LiteralPath '${escaped}'){exit 1}`;
  requireSuccess(
    spawnSync(nativePowerShellPath(), ['-NoProfile', '-NonInteractive', '-Command', command], {
      encoding: 'utf8',
      timeout: 30_000,
      windowsHide: true,
    }),
    'protected stale-cleanup fixture removal',
  );
}

function nativePowerShellPath() {
  return resolve(
    process.env.WINDIR ?? String.raw`C:\Windows`,
    'System32',
    'WindowsPowerShell',
    'v1.0',
    'powershell.exe',
  );
}

function windowsCSharpCompiler() {
  return resolve(
    process.env.WINDIR ?? String.raw`C:\Windows`,
    'Microsoft.NET',
    'Framework64',
    'v4.0.30319',
    'csc.exe',
  );
}

async function writeFixtureBootstrapInclude(workDirectory, expectedNsisRoot) {
  const projectRoot = join(workDirectory, 'fixture-project');
  const projectDirectory = join(projectRoot, 'app');
  const buildDirectory = join(projectRoot, 'build');
  await Promise.all([
    mkdir(projectDirectory, { recursive: true }),
    mkdir(buildDirectory, { recursive: true }),
  ]);
  const productionSource = await readFile(protectedBootstrapSourcePath, 'utf8');
  const expectedRootLine = "$expectedNsisRoot = Join-Path $nativeProgramFiles 'Talking Quill'";
  const fixtureRoot = expectedNsisRoot.replaceAll("'", "''");
  const bootstrapErrorLog = join(workDirectory, 'bootstrap-errors.txt').replaceAll("'", "''");
  const fixtureSource = productionSource
    .replace(expectedRootLine, `$expectedNsisRoot = '${fixtureRoot}'`)
    .replace(
      "[Console]::Error.WriteLine('Protected bootstrap failed: ' + $_.Exception.Message)",
      `[IO.File]::AppendAllText('${bootstrapErrorLog}', $_.Exception.ToString() + [Environment]::NewLine)`,
    );
  if (fixtureSource === productionSource) {
    throw new Error('Protected bootstrap fixture root anchor is missing');
  }
  const [installValidation, cleanupFixture] = await Promise.all([
    readFile(resolve(repositoryRoot, 'build', 'installer-install-validation.nsh')),
    readFile(resolve(repositoryRoot, 'build', 'windows-personal-machine-cleanup.ps1')),
  ]);
  await Promise.all([
    writeFile(
      join(buildDirectory, 'windows-protected-bootstrap-encoded.nsh'),
      renderProtectedBootstrapInclude(fixtureSource),
      'utf8',
    ),
    writeFile(join(buildDirectory, 'installer-install-validation.nsh'), installValidation),
    writeFile(join(buildDirectory, 'windows-personal-machine-cleanup.ps1'), cleanupFixture),
  ]);
  return projectDirectory;
}

function requireBootstrapPayload(source) {
  const match = source.match(
    /^!define TALKING_QUILL_PROTECTED_BOOTSTRAP_PAYLOAD "([A-Za-z0-9+/]+={0,2})"$/mu,
  );
  if (match?.[1] === undefined) throw new Error('Static protected bootstrap payload is malformed');
  return match[1];
}

async function runCompiledNsisRuntimeHarness(workDirectory, uninstallerRoot) {
  const installer = join(workDirectory, 'installer-macro-check.exe');
  const uninstallerWriter = join(workDirectory, 'uninstaller-macro-check.exe');
  const uninstaller = join(workDirectory, 'unused-uninstaller.exe');
  requireStatus(
    spawnSupervisedWithArguments(uninstallerWriter, ['/S']),
    0,
    'compiled uninstaller fixture writer',
  );
  await readFile(uninstaller);

  const publicArguments = [
    '/S',
    '--uninstall',
    "--fixture-path=C:\\A path\\O'Brien\\payload.exe",
    '--uninstall-note=remove user data',
  ];
  for (const [label, executable] of [
    ['installer', installer],
    ['uninstaller', uninstaller],
  ]) {
    const logPath = join(workDirectory, `${label}-runtime.txt`);
    const invocationArguments = [...publicArguments, '/TQELEVATEDBOOTSTRAP=1'];
    let rawArguments = invocationArguments.map(quoteWindowsArgument).join(' ');
    if (label === 'uninstaller') rawArguments += ` _?=${nsisPath(uninstallerRoot)}`;
    const result = await runResidueCheckedCase(`compiled ${label} success`, () =>
      spawnWithRawArguments(executable, rawArguments, {
        ...process.env,
        TQ_BOOTSTRAP_TEST_LOG: logPath,
      }),
    );
    if (result.status !== 37) {
      try {
        result.stderr += `\n${await readFile(join(workDirectory, 'bootstrap-errors.txt'), 'utf8')}`;
      } catch (error) {
        if (error?.code !== 'ENOENT') throw error;
      }
    }
    requireStatus(result, 37, `compiled ${label} protected bootstrap`);
    requireForwardedNsisParameters(
      await readFile(logPath, 'utf8'),
      publicArguments,
      label,
      uninstallerRoot,
    );
  }

  await requireCompiledNsisTailRejection({ uninstaller, workDirectory, uninstallerRoot });

  await requireCompiledTempRejection({
    installer,
    uninstaller,
    workDirectory,
    uninstallerRoot,
    arguments_: ['/TQELEVATEDBOOTSTRAP=1', String.raw`/TQPROTECTEDTEMP=C:\malformed`],
    environment: { TEMP: String.raw`C:\malformed`, TMP: String.raw`C:\malformed` },
    caseName: 'malformed',
  });

  const programData = nativeProgramDataPath();
  const reparseTarget = join(workDirectory, 'compiled-reparse-target');
  const reparseLeaf = join(programData, `.Talking Quill.Harness-${randomHarnessSuffix()}`);
  await mkdir(reparseTarget, { recursive: true });
  try {
    await symlink(reparseTarget, reparseLeaf, 'junction');
    await requireCompiledTempRejection({
      installer,
      uninstaller,
      workDirectory,
      uninstallerRoot,
      arguments_: ['/TQELEVATEDBOOTSTRAP=1', `/TQPROTECTEDTEMP=${reparseLeaf}`],
      environment: { TEMP: reparseLeaf, TMP: reparseLeaf },
      caseName: 'reparse',
    });
  } finally {
    await removeHarnessReparseLeaf(reparseLeaf);
  }
}

async function requireCompiledNsisTailRejection({ uninstaller, workDirectory, uninstallerRoot }) {
  const unrelated = join(workDirectory, 'unrelated-uninstall-root');
  const reparseTarget = join(workDirectory, 'uninstaller-root-reparse-target');
  await Promise.all([
    mkdir(unrelated, { recursive: true }),
    mkdir(reparseTarget, { recursive: true }),
  ]);
  const rejected = [
    unrelated,
    `${uninstallerRoot}\\..\\unrelated-uninstall-root`,
    `${uninstallerRoot}-alternate`,
  ];
  for (const [index, tail] of rejected.entries()) {
    const logPath = join(workDirectory, `uninstaller-tail-${String(index)}.txt`);
    const result = await runResidueCheckedCase(`forged NSIS tail ${String(index)}`, () =>
      spawnWithRawArguments(uninstaller, `/TQELEVATEDBOOTSTRAP=1 _?=${nsisPath(tail)}`, {
        ...process.env,
        TQ_BOOTSTRAP_TEST_LOG: logPath,
      }),
    );
    requireStatus(result, 78, `compiled uninstaller forged _?= rejection ${String(index)}`);
    await requireMissing(logPath, 'compiled uninstaller reached runtime with forged _?=');
  }

  await rm(uninstallerRoot, { recursive: true });
  try {
    await symlink(reparseTarget, uninstallerRoot, 'junction');
    const logPath = join(workDirectory, 'uninstaller-tail-reparse.txt');
    const result = await runResidueCheckedCase('reparse NSIS tail', () =>
      spawnWithRawArguments(uninstaller, `/TQELEVATEDBOOTSTRAP=1 _?=${nsisPath(uninstallerRoot)}`, {
        ...process.env,
        TQ_BOOTSTRAP_TEST_LOG: logPath,
      }),
    );
    requireStatus(result, 78, 'compiled uninstaller reparse _?= rejection');
    await requireMissing(logPath, 'compiled uninstaller reached runtime with reparse _?=');
  } finally {
    await rm(uninstallerRoot, { recursive: false, force: true });
    await mkdir(uninstallerRoot);
  }
}

async function requireMissing(path, message) {
  try {
    await readFile(path);
    throw new Error(message);
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
}

async function requireCompiledTempRejection({
  installer,
  uninstaller,
  workDirectory,
  uninstallerRoot,
  arguments_,
  environment,
  caseName,
}) {
  for (const [label, executable] of [
    ['installer', installer],
    ['uninstaller', uninstaller],
  ]) {
    const logPath = join(workDirectory, `${label}-${caseName}.txt`);
    const [elevatedMarker, protectedMarker] = arguments_;
    const protectedPath = protectedMarker.slice('/TQPROTECTEDTEMP='.length);
    let rawArguments = `${quoteWindowsArgument(elevatedMarker)} /TQPROTECTEDTEMP=${quoteWindowsArgument(protectedPath)}`;
    if (label === 'uninstaller') {
      rawArguments += ` _?=${nsisPath(uninstallerRoot)}`;
    }
    const result = await runResidueCheckedCase(`compiled ${label} ${caseName} rejection`, () =>
      spawnWithRawArguments(executable, rawArguments, {
        ...process.env,
        ...environment,
        TQ_BOOTSTRAP_TEST_LOG: logPath,
      }),
    );
    requireStatus(result, 78, `compiled ${label} ${caseName} TEMP rejection`);
    await requireMissing(
      logPath,
      `compiled ${label} reached runtime after ${caseName} TEMP rejection`,
    );
  }
}

function requireForwardedNsisParameters(evidence, publicArguments, label, uninstallerRoot) {
  const [temp, tmp, instDir, ...parameterLines] = evidence.split(/\r?\n/u);
  let parameters = parameterLines.join('\n');
  const nsisTail = parameters.lastIndexOf(' _?=');
  if (nsisTail >= 0) parameters = parameters.slice(0, nsisTail);
  const observed = parseWindowsCommandLine(`fixture.exe ${parameters}`).slice(1);
  const publicObserved = observed.filter(
    (argument) =>
      !argument.toUpperCase().startsWith('/TQELEVATEDBOOTSTRAP=') &&
      !argument.toUpperCase().startsWith('/TQPROTECTEDTEMP=') &&
      !argument.startsWith('_?='),
  );
  const elevated = observed.filter(
    (argument) => argument.toUpperCase() === '/TQELEVATEDBOOTSTRAP=1',
  );
  const protectedMarkers = observed.filter((argument) =>
    argument.toUpperCase().startsWith('/TQPROTECTEDTEMP='),
  );
  if (
    temp === undefined ||
    tmp === undefined ||
    temp !== tmp ||
    (label === 'uninstaller' && instDir?.toLowerCase() !== uninstallerRoot.toLowerCase()) ||
    JSON.stringify(publicObserved) !== JSON.stringify(publicArguments) ||
    elevated.length !== 1 ||
    protectedMarkers.length !== 1 ||
    protectedMarkers[0]?.slice('/TQPROTECTEDTEMP='.length) !== temp
  ) {
    throw new Error(
      `compiled ${label} bootstrap changed argv or protected TEMP: ${JSON.stringify({ temp, tmp, instDir, observed, publicObserved })}`,
    );
  }
}

function spawnWithRawArguments(executable, rawArguments, environment, timeout = 60_000) {
  if (windowsJobSupervisor === undefined) {
    throw new Error('Windows Job Object supervisor is unavailable');
  }
  return spawnSync(
    windowsJobSupervisor,
    [String(timeout), executable, Buffer.from(rawArguments, 'utf8').toString('base64')],
    {
      encoding: 'utf8',
      env: environment,
      timeout: timeout + 20_000,
      windowsHide: true,
    },
  );
}

function spawnSupervisedWithArguments(executable, arguments_, environment = process.env, timeout) {
  return spawnWithRawArguments(
    executable,
    arguments_.map(quoteWindowsArgument).join(' '),
    environment,
    timeout,
  );
}

function quoteWindowsArgument(argument) {
  if (argument !== '' && !/[\s"]/u.test(argument)) return argument;
  let rendered = '"';
  let slashes = 0;
  for (const character of argument) {
    if (character === '\\') {
      slashes++;
    } else if (character === '"') {
      rendered += '\\'.repeat(slashes * 2 + 1) + '"';
      slashes = 0;
    } else {
      rendered += '\\'.repeat(slashes) + character;
      slashes = 0;
    }
  }
  return `${rendered}${'\\'.repeat(slashes * 2)}"`;
}

function parseWindowsCommandLine(commandLine) {
  const arguments_ = [];
  let offset = 0;
  while (offset < commandLine.length) {
    while (offset < commandLine.length && /[ \t]/u.test(commandLine[offset])) offset++;
    if (offset === commandLine.length) break;
    let argument = '';
    let quoted = false;
    while (offset < commandLine.length) {
      let slashes = 0;
      while (commandLine[offset] === '\\') {
        slashes++;
        offset++;
      }
      if (commandLine[offset] === '"') {
        argument += '\\'.repeat(Math.floor(slashes / 2));
        if (slashes % 2 === 1) argument += '"';
        else quoted = !quoted;
        offset++;
        continue;
      }
      argument += '\\'.repeat(slashes);
      if (offset === commandLine.length || (!quoted && /[ \t]/u.test(commandLine[offset]))) break;
      argument += commandLine[offset];
      offset++;
    }
    arguments_.push(argument);
  }
  return arguments_;
}

async function runProtectedBootstrapIntegration(workDirectory, payload) {
  const fixtureDirectory = join(workDirectory, "fixture path's bootstrap");
  await mkdir(fixtureDirectory, { recursive: true });
  const sourcePath = join(fixtureDirectory, 'BootstrapFixture.cs');
  const executable = join(fixtureDirectory, "Harmless Parent's Fixture.exe");
  const logPath = join(fixtureDirectory, 'child-arguments.txt');
  await writeFile(sourcePath, bootstrapFixtureSource(), 'utf8');
  const compiler = resolve(
    process.env.WINDIR ?? String.raw`C:\Windows`,
    'Microsoft.NET',
    'Framework64',
    'v4.0.30319',
    'csc.exe',
  );
  requireSuccess(
    spawnSync(compiler, ['/nologo', `/out:${executable}`, sourcePath], {
      encoding: 'utf8',
      timeout: 60_000,
      windowsHide: true,
    }),
    'protected bootstrap fixture compilation',
  );

  const powershell = resolve(
    process.env.WINDIR ?? String.raw`C:\Windows`,
    'System32',
    'WindowsPowerShell',
    'v1.0',
    'powershell.exe',
  );
  const command = protectedBootstrapCommand(payload);
  const publicArguments = [
    '/S',
    '--uninstall',
    "--fixture-path=C:\\A path\\O'Brien\\payload.exe",
    "/DELETEAPPDATA=O'Brien value",
    '/EXITCODE=37',
  ];
  const success = await runResidueCheckedCase('C# protected bootstrap success', () =>
    spawnSupervisedWithArguments(executable, [...publicArguments, '/TQELEVATEDBOOTSTRAP=1'], {
      ...process.env,
      TQ_BOOTSTRAP_TEST_COMMAND: command,
      TQ_BOOTSTRAP_TEST_LOG: logPath,
      TQ_BOOTSTRAP_TEST_POWERSHELL: powershell,
      TQ_BOOTSTRAP_FIXTURE_STAGE: '',
    }),
  );
  if (success.error || success.status !== 37) {
    throw new Error(
      `protected bootstrap did not propagate fixture exit 37${success.error ? `: ${success.error.message}` : ''}\n${success.stdout ?? ''}\n${success.stderr ?? ''}`,
    );
  }
  const lines = (await readFile(logPath, 'utf8')).trim().split(/\r?\n/u);
  const observedArguments = lines
    .slice(2)
    .map((value) => Buffer.from(value, 'base64').toString('utf8'));
  if (
    lines[0] !== 'protected=true' ||
    lines[1] !== 'same-temp=true' ||
    JSON.stringify(observedArguments.slice(0, publicArguments.length)) !==
      JSON.stringify(publicArguments) ||
    observedArguments.filter((value) => value.toUpperCase() === '/TQELEVATEDBOOTSTRAP=1').length !==
      1 ||
    !observedArguments.at(-1)?.startsWith('/TQPROTECTEDTEMP=')
  ) {
    throw new Error('protected bootstrap changed public argv or failed to set protected TEMP');
  }

  const relativeNsisTail = await runResidueCheckedCase('relative NSIS tail rejection', () =>
    spawnBootstrapFixture(
      executable,
      command,
      powershell,
      ['/TQELEVATEDBOOTSTRAP=1', '_?=..\\relative-uninstall-root'],
      {},
    ),
  );
  if (relativeNsisTail.status !== 78) {
    throw new Error(
      `protected bootstrap accepted relative NSIS tail: ${String(relativeNsisTail.status)}`,
    );
  }

  const malformed = await runResidueCheckedCase('malformed TEMP rejection', () =>
    spawnBootstrapFixture(executable, command, powershell, ['/TQPROTECTEDTEMP=C:\\malformed'], {
      TEMP: String.raw`C:\malformed`,
      TMP: String.raw`C:\malformed`,
    }),
  );
  if (malformed.status !== 78) {
    throw new Error(`protected bootstrap accepted malformed TEMP: ${String(malformed.status)}`);
  }

  for (const [label, arguments_] of [
    ['duplicate elevated marker', ['/TQELEVATEDBOOTSTRAP=1', '/TQELEVATEDBOOTSTRAP=1']],
    [
      'duplicate protected marker',
      ['/TQELEVATEDBOOTSTRAP=1', '/TQPROTECTEDTEMP=C:\\one', '/TQPROTECTEDTEMP=C:\\two'],
    ],
  ]) {
    const duplicate = await runResidueCheckedCase(`${label} rejection`, () =>
      spawnBootstrapFixture(executable, command, powershell, arguments_, {}),
    );
    if (duplicate.status !== 78) {
      throw new Error(`bootstrap fixture accepted ${label}: ${String(duplicate.status)}`);
    }
  }

  const programData = nativeProgramDataPath();
  const reparseTarget = join(fixtureDirectory, 'reparse-target');
  const reparseLeaf = join(programData, `.Talking Quill.Harness-${randomHarnessSuffix()}`);
  await mkdir(reparseTarget, { recursive: true });
  try {
    await symlink(reparseTarget, reparseLeaf, 'junction');
    const reparse = await runResidueCheckedCase('reparse TEMP rejection', () =>
      spawnBootstrapFixture(executable, command, powershell, [`/TQPROTECTEDTEMP=${reparseLeaf}`], {
        TEMP: reparseLeaf,
        TMP: reparseLeaf,
      }),
    );
    if (reparse.status !== 78) {
      throw new Error(`protected bootstrap accepted reparse TEMP: ${String(reparse.status)}`);
    }
  } finally {
    await removeHarnessReparseLeaf(reparseLeaf);
  }
}

function nativeProgramDataPath() {
  const result = spawnSync(
    nativePowerShellPath(),
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      '[Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)',
    ],
    { encoding: 'utf8', timeout: 30_000, windowsHide: true },
  );
  requireSuccess(result, 'native ProgramData lookup');
  const path = result.stdout.trim();
  if (!/^[A-Za-z]:\\/u.test(path)) throw new Error('Native ProgramData lookup was malformed');
  return path;
}

function randomHarnessSuffix() {
  return randomBytes(16).toString('hex');
}

async function removeHarnessReparseLeaf(path) {
  for (let attempt = 0; attempt < 20; attempt++) {
    try {
      await rm(path, { recursive: false, force: true });
      return;
    } catch (error) {
      if (attempt === 19) throw error;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
}

async function runResidueCheckedCase(label, action) {
  const starting = await snapshotProtectedBootstrapResidue();
  try {
    return await action();
  } finally {
    await assertProtectedBootstrapResidueUnchanged(starting, label);
  }
}

async function snapshotProtectedBootstrapResidue() {
  const programData = nativeProgramDataPath();
  const names = (await readdir(programData)).filter((name) =>
    /^\.Talking Quill\.(?:Installer|Harness|Cleanup)-[0-9a-f]{32}$/u.test(name),
  );
  return { programData, names: names.sort() };
}

async function assertProtectedBootstrapResidueUnchanged(starting, label) {
  for (let observation = 1; observation <= 3; observation++) {
    const ending = await snapshotProtectedBootstrapResidue();
    if (
      ending.programData.toLowerCase() !== starting.programData.toLowerCase() ||
      JSON.stringify(ending.names) !== JSON.stringify(starting.names)
    ) {
      const before = new Set(starting.names);
      const after = new Set(ending.names);
      const added = ending.names.filter((name) => !before.has(name));
      const removed = starting.names.filter((name) => !after.has(name));
      throw new Error(
        `${label} changed protected-bootstrap ProgramData residue on observation ${String(observation)}: ${JSON.stringify({ added, removed, preExistingCount: starting.names.length })}`,
      );
    }
    if (observation !== 3) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
}

function spawnBootstrapFixture(executable, command, powershell, arguments_, environment) {
  return spawnSupervisedWithArguments(executable, arguments_, {
    ...process.env,
    ...environment,
    TQ_BOOTSTRAP_TEST_COMMAND: command,
    TQ_BOOTSTRAP_TEST_POWERSHELL: powershell,
    TQ_BOOTSTRAP_FIXTURE_STAGE: '',
  });
}

function protectedBootstrapCommand(payload) {
  const script = `$b='${payload}';$m=New-Object IO.MemoryStream(,[Convert]::FromBase64String($b));$z=New-Object IO.Compression.GZipStream($m,[IO.Compression.CompressionMode]::Decompress);$r=New-Object IO.StreamReader($z,[Text.Encoding]::UTF8);&([ScriptBlock]::Create($r.ReadToEnd()))`;
  return `-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "${script}"`;
}

function requireSuccess(result, label) {
  requireStatus(result, 0, label);
}

function requireStatus(result, expected, label) {
  if (result.error || result.status !== expected) {
    throw new Error(
      `${label} failed with ${String(result.status)}, expected ${String(expected)}${result.error ? `: ${result.error.message}` : ''}\n${result.stdout ?? ''}\n${result.stderr ?? ''}`,
    );
  }
}

function jobSupervisorFixtureSource() {
  return String.raw`using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Threading;

internal static class JobSupervisorFixture
{
    private static int Main(string[] args)
    {
        if (args.Length == 2 && args[0] == "child")
        {
            File.WriteAllText(args[1], Process.GetCurrentProcess().Id.ToString());
            Thread.Sleep(Timeout.Infinite);
            return 0;
        }
        if (args.Length != 2) return 90;
        ProcessStartInfo start = new ProcessStartInfo();
        start.FileName = Assembly.GetExecutingAssembly().Location;
        start.Arguments = "child \"" + args[1].Replace("\"", "\\\"") + "\"";
        start.UseShellExecute = false;
        Process.Start(start);
        DateTime deadline = DateTime.UtcNow.AddSeconds(5);
        while (!File.Exists(args[1]) && DateTime.UtcNow < deadline) Thread.Sleep(10);
        if (!File.Exists(args[1])) return 91;
        if (args[0] == "timeout") Thread.Sleep(Timeout.Infinite);
        return 23;
    }
}`;
}

function bootstrapFixtureSource() {
  return String.raw`using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text;

internal static class BootstrapFixture
{
    private static int Main(string[] args)
    {
        if (Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_FIXTURE_STAGE") == "waiting")
        {
            string[] elevated = args.Where(value => value.Equals(
                "/TQELEVATEDBOOTSTRAP=1", StringComparison.OrdinalIgnoreCase)).ToArray();
            string[] elevatedFamily = args.Where(value => value.StartsWith(
                "/TQELEVATEDBOOTSTRAP=", StringComparison.OrdinalIgnoreCase)).ToArray();
            string[] protectedFamily = args.Where(value => value.StartsWith(
                "/TQPROTECTEDTEMP=", StringComparison.OrdinalIgnoreCase)).ToArray();
            if (elevated.Length != 1 || elevatedFamily.Length != 1 ||
                protectedFamily.Length != 1 ||
                protectedFamily[0].Length == "/TQPROTECTEDTEMP=".Length) return 78;

            string marker = protectedFamily[0];
            string temp = Environment.GetEnvironmentVariable("TEMP");
            if (!String.Equals(marker.Substring(marker.IndexOf('=') + 1), temp,
                StringComparison.Ordinal)) return 78;
            string log = Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_TEST_LOG");
            if (!String.IsNullOrEmpty(log))
            {
                string[] lines = new[] { "protected=true", "same-temp=true" }
                    .Concat(args.Select(value => Convert.ToBase64String(
                        Encoding.UTF8.GetBytes(value)))).ToArray();
                File.WriteAllLines(log, lines, new UTF8Encoding(false));
            }
            string exit = args.FirstOrDefault(value => value.StartsWith(
                "/EXITCODE=", StringComparison.Ordinal));
            return exit == null ? 0 : Int32.Parse(exit.Substring(10));
        }

        Environment.SetEnvironmentVariable("TQ_BOOTSTRAP_FIXTURE_STAGE", "waiting");
        return RunBootstrap();
    }

    private static int RunBootstrap()
    {
        ProcessStartInfo start = new ProcessStartInfo();
        start.FileName = Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_TEST_POWERSHELL");
        start.Arguments = Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_TEST_COMMAND");
        start.UseShellExecute = false;
        Process child = Process.Start(start);
        child.WaitForExit();
        return child.ExitCode;
    }
}`;
}

function macroCompileScript({ mode, outputPath, projectDirectory }) {
  const uninstall = mode === 'uninstaller';
  return `Unicode true
Name "Talking Quill early-init ${mode} check"
OutFile "${nsisPath(outputPath)}"
RequestExecutionLevel user
!define APP_GUID "early-init-check"
!define UNINSTALL_APP_KEY "early-init-check"
!define APP_FILENAME "Talking Quill"
!define PROJECT_DIR "${nsisPath(projectDirectory)}"
!define TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD
${uninstall ? '!define BUILD_UNINSTALLER\n' : ''}!include "${nsisPath(installerInclude)}"

Section
${
  uninstall
    ? `  WriteUninstaller "${nsisPath(join(dirname(outputPath), 'unused-uninstaller.exe'))}"\n`
    : runtimeEvidenceSection()
}SectionEnd
${uninstall ? uninstallerOuterContext() : installerOuterContext()}
`;
}

function installerOuterContext() {
  return `Function .onInit
  !insertmacro customEarlyInit
  !insertmacro customInit
FunctionEnd

Function .onUserAbort
  Call TalkingQuillOnUserAbort
FunctionEnd`;
}

function runtimeEvidenceSection({ quit = false } = {}) {
  return `  ReadEnvStr $R8 "TQ_BOOTSTRAP_TEST_LOG"
  ReadEnvStr $R6 "TMP"
  \${GetParameters} $R9
  FileOpen $R7 "$R8" w
  FileWrite $R7 "$TEMP$\\r$\\n$R6$\\r$\\n$INSTDIR$\\r$\\n$R9"
  FileClose $R7
  SetErrorLevel 37
${quit ? '  Quit\n' : ''}`;
}

function uninstallerOuterContext() {
  return `UninstPage custom un.TalkingQuillDataPage un.TalkingQuillDataPageLeave
UninstPage instfiles

Section "Uninstall"
${runtimeEvidenceSection()}SectionEnd

Function un.onInit
  !insertmacro customUnEarlyInit
  !insertmacro customUnInit
${runtimeEvidenceSection({ quit: true })}FunctionEnd`;
}

function nsisPath(path) {
  return process.platform === 'win32' ? path.replaceAll('/', '\\') : path;
}
