import { spawnSync } from 'node:child_process';
import { gzipSync } from 'node:zlib';
import { createRequire } from 'node:module';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

const repositoryRoot = resolve(import.meta.dirname, '..');
const temporaryRoot = resolve(repositoryRoot, 'tmp');
const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const appBuilderRoot = dirname(electronBuilderRequire.resolve('app-builder-lib/package.json'));
const { getMakeNsisPath } = electronBuilderRequire(
  join(appBuilderRoot, 'out', 'toolsets', 'windows.js'),
);

const installerInclude = resolve(repositoryRoot, 'build', 'installer.nsh');
const bootstrapSourcePath = resolve(repositoryRoot, 'build', 'windows-protected-bootstrap.ps1');
const bootstrapPayloadPath = resolve(
  repositoryRoot,
  'build',
  'windows-protected-bootstrap-encoded.nsh',
);
const projectDirectory = resolve(repositoryRoot, 'app');
const makeNsisArguments = ['-WX', '-V2', '-NOCD'];
const [source, bootstrapSource, bootstrapPayloadInclude] = await Promise.all([
  readFile(installerInclude, 'utf8'),
  readFile(bootstrapSourcePath),
  readFile(bootstrapPayloadPath, 'utf8'),
]);
const bootstrapPayload = requireBootstrapPayload(bootstrapPayloadInclude);
if (gzipSync(bootstrapSource, { level: 9 }).toString('base64') !== bootstrapPayload) {
  throw new Error('Static protected bootstrap payload is stale');
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
  const makensis = await getMakeNsisPath(undefined);
  for (const mode of ['installer', 'uninstaller']) {
    const scriptPath = join(workDirectory, `${mode}.nsi`);
    const outputPath = join(workDirectory, `${mode}-macro-check.exe`);
    await writeFile(scriptPath, macroCompileScript({ mode, outputPath }), 'utf8');

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
  }
  if (process.platform === 'win32') {
    await runCompiledNsisRuntimeHarness(workDirectory);
    await runProtectedBootstrapIntegration(workDirectory, bootstrapPayload);
  }
} finally {
  await rm(workDirectory, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
}

function requireBootstrapPayload(source) {
  const match = source.match(
    /^!define TALKING_QUILL_PROTECTED_BOOTSTRAP_PAYLOAD "([A-Za-z0-9+/]+={0,2})"$/mu,
  );
  if (match?.[1] === undefined) throw new Error('Static protected bootstrap payload is malformed');
  return match[1];
}

async function runCompiledNsisRuntimeHarness(workDirectory) {
  const logPath = join(workDirectory, 'compiled-nsis-runtime.txt');
  const environment = { ...process.env, TQ_BOOTSTRAP_TEST_LOG: logPath };
  const publicArguments = ['/S', '--uninstall', "--fixture-path=C:\\A path\\O'Brien\\payload.exe"];
  const bootstrapMarker = '/TQELEVATEDBOOTSTRAP=1';
  const installer = join(workDirectory, 'installer-macro-check.exe');
  const installed = spawnSync(
    installer,
    [...publicArguments, "/DELETEAPPDATA=O'Brien value", bootstrapMarker],
    {
      encoding: 'utf8',
      env: environment,
      timeout: 60_000,
      windowsHide: true,
    },
  );
  requireStatus(installed, 37, 'compiled installer protected bootstrap');
  requireForwardedNsisParameters(await readFile(logPath, 'utf8'), true);
}

function requireForwardedNsisParameters(parameters, expectDeleteArgument) {
  const expectedFragments = ['/S', '--uninstall', "O'Brien"];
  if (expectDeleteArgument) expectedFragments.push('/DELETEAPPDATA=');
  for (const expected of expectedFragments) {
    if (!parameters.includes(expected)) {
      throw new Error(`compiled NSIS bootstrap dropped public argument fragment: ${expected}`);
    }
  }
  if (
    parameters.match(/\/TQELEVATEDBOOTSTRAP=1/giu)?.length !== 1 ||
    !parameters.includes('/TQPROTECTEDTEMP=')
  ) {
    throw new Error('compiled NSIS bootstrap marker forwarding is invalid');
  }
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
  const success = spawnSync(executable, [...publicArguments, '/TQELEVATEDBOOTSTRAP=1'], {
    encoding: 'utf8',
    env: {
      ...process.env,
      TQ_BOOTSTRAP_TEST_COMMAND: command,
      TQ_BOOTSTRAP_TEST_LOG: logPath,
      TQ_BOOTSTRAP_TEST_POWERSHELL: powershell,
    },
    timeout: 60_000,
    windowsHide: true,
  });
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

  const malformed = spawnBootstrapFixture(
    executable,
    command,
    powershell,
    ['/TQPROTECTEDTEMP=C:\\malformed'],
    {
      TEMP: String.raw`C:\malformed`,
      TMP: String.raw`C:\malformed`,
    },
  );
  if (malformed.status !== 78) {
    throw new Error(`protected bootstrap accepted malformed TEMP: ${String(malformed.status)}`);
  }

  const programData = process.env.ProgramData ?? String.raw`C:\ProgramData`;
  const reparseTarget = join(fixtureDirectory, 'reparse-target');
  const reparseLeaf = join(programData, `.Talking Quill.Installer-test-${process.pid}`);
  await mkdir(reparseTarget);
  try {
    await symlink(reparseTarget, reparseLeaf, 'junction');
    const reparse = spawnBootstrapFixture(
      executable,
      command,
      powershell,
      [`/TQPROTECTEDTEMP=${reparseLeaf}`],
      { TEMP: reparseLeaf, TMP: reparseLeaf },
    );
    if (reparse.status !== 78) {
      throw new Error(`protected bootstrap accepted reparse TEMP: ${String(reparse.status)}`);
    }
  } finally {
    await rm(reparseLeaf, { recursive: false, force: true });
  }
}

function spawnBootstrapFixture(executable, command, powershell, arguments_, environment) {
  return spawnSync(executable, arguments_, {
    encoding: 'utf8',
    env: {
      ...process.env,
      ...environment,
      TQ_BOOTSTRAP_TEST_COMMAND: command,
      TQ_BOOTSTRAP_TEST_POWERSHELL: powershell,
    },
    timeout: 60_000,
    windowsHide: true,
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
        string waiting = Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_TEST_WAITING_PID");
        if (!String.IsNullOrEmpty(waiting) && waiting != Process.GetCurrentProcess().Id.ToString())
        {
            int validation = RunBootstrap();
            if (validation != 0) return validation;
            string marker = args.FirstOrDefault(value => value.StartsWith(
                "/TQPROTECTEDTEMP=", StringComparison.OrdinalIgnoreCase));
            string temp = Environment.GetEnvironmentVariable("TEMP");
            string log = Environment.GetEnvironmentVariable("TQ_BOOTSTRAP_TEST_LOG");
            if (!String.IsNullOrEmpty(log))
            {
                string[] lines = new[] {
                    "protected=" + (!String.IsNullOrEmpty(marker)).ToString().ToLowerInvariant(),
                    "same-temp=" + (marker != null && marker.Substring(marker.IndexOf('=') + 1) == temp).ToString().ToLowerInvariant()
                }.Concat(args.Select(value => Convert.ToBase64String(Encoding.UTF8.GetBytes(value)))).ToArray();
                File.WriteAllLines(log, lines, new UTF8Encoding(false));
            }
            string exit = args.FirstOrDefault(value => value.StartsWith("/EXITCODE=", StringComparison.Ordinal));
            return exit == null ? 0 : Int32.Parse(exit.Substring(10));
        }

        Environment.SetEnvironmentVariable(
            "TQ_BOOTSTRAP_TEST_WAITING_PID",
            Process.GetCurrentProcess().Id.ToString());
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

function macroCompileScript({ mode, outputPath }) {
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

function runtimeEvidenceSection() {
  return `  ReadEnvStr $R8 "TQ_BOOTSTRAP_TEST_LOG"
  \${GetParameters} $R9
  FileOpen $R7 "$R8" w
  FileWrite $R7 "$R9"
  FileClose $R7
  SetErrorLevel 37
`;
}

function uninstallerOuterContext() {
  return `UninstPage custom un.TalkingQuillDataPage un.TalkingQuillDataPageLeave
UninstPage instfiles

Section "Uninstall"
${runtimeEvidenceSection()}SectionEnd

Function un.onInit
  !insertmacro customUnEarlyInit
  !insertmacro customUnInit
FunctionEnd`;
}

function nsisPath(path) {
  return process.platform === 'win32' ? path.replaceAll('/', '\\') : path;
}
