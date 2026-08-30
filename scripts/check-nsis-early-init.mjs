import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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
const projectDirectory = resolve(repositoryRoot, 'app');
const makeNsisArguments = ['-WX', '-V2', '-NOCD'];
const source = await readFile(installerInclude, 'utf8');

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
} finally {
  await rm(workDirectory, { recursive: true, force: true });
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
${uninstall ? `  WriteUninstaller "${nsisPath(join(dirname(outputPath), 'unused-uninstaller.exe'))}"\n` : ''}SectionEnd
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

function uninstallerOuterContext() {
  return `UninstPage custom un.TalkingQuillDataPage un.TalkingQuillDataPageLeave
UninstPage instfiles

Section "Uninstall"
SectionEnd

Function un.onInit
  !insertmacro customUnEarlyInit
  !insertmacro customUnInit
FunctionEnd`;
}

function nsisPath(path) {
  return process.platform === 'win32' ? path.replaceAll('/', '\\') : path;
}
