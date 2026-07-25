import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);

export function validateNsisUninstallPolicy({ custom, assisted, uninstaller }) {
  const welcomeMacro = macroBody(custom, 'customUnWelcomePage');
  if (!welcomeMacro.includes('MUI_UNPAGE_WELCOME') || !welcomeMacro.includes('UninstPage custom')) {
    throw new Error('NSIS data opt-in page must occupy the pre-InstFiles uninstall welcome slot');
  }
  if (custom.includes('!macro customUninstallPage')) {
    throw new Error('NSIS customUninstallPage runs after InstFiles and is forbidden');
  }
  const pageHook = assisted.indexOf('!ifmacrodef customUnWelcomePage');
  const instFilesPage = assisted.indexOf('!insertmacro MUI_UNPAGE_INSTFILES');
  if (pageHook < 0 || instFilesPage < 0 || pageHook >= instFilesPage) {
    throw new Error('Pinned NSIS template no longer places customUnWelcomePage before InstFiles');
  }

  const customInstall = uninstaller.indexOf('!insertmacro customUnInstall');
  const installedDeletion = uninstaller.indexOf('# delete the installed files');
  if (customInstall < 0 || installedDeletion < 0 || customInstall >= installedDeletion) {
    throw new Error('Pinned NSIS template no longer runs reset helper before install deletion');
  }
  const resetHelper = custom.indexOf('ExecWait');
  const resetAbort = custom.indexOf('Abort', resetHelper);
  if (
    resetHelper < 0 ||
    resetAbort < 0 ||
    !custom.includes('IfFileExists "$INSTDIR\\${APP_FILENAME}.exe"')
  ) {
    throw new Error('NSIS reset helper must be present and fail closed before uninstall deletion');
  }
}

export async function validateInstalledNsisTemplates() {
  const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
  const appBuilderRoot = dirname(electronBuilderRequire.resolve('app-builder-lib/package.json'));
  const templateRoot = resolve(appBuilderRoot, 'templates/nsis');
  const [custom, assisted, uninstaller] = await Promise.all([
    readFile(resolve(repositoryRoot, 'build/installer.nsh'), 'utf8'),
    readFile(resolve(templateRoot, 'assistedInstaller.nsh'), 'utf8'),
    readFile(resolve(templateRoot, 'uninstaller.nsh'), 'utf8'),
  ]);
  validateNsisUninstallPolicy({ custom, assisted, uninstaller });
}

function macroBody(source, name) {
  const start = source.indexOf(`!macro ${name}`);
  if (start < 0) throw new Error(`Missing NSIS macro ${name}`);
  const end = source.indexOf('!macroend', start);
  if (end < 0) throw new Error(`Unterminated NSIS macro ${name}`);
  return source.slice(start, end);
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  await validateInstalledNsisTemplates();
  console.log('Pinned extracted NSIS uninstall page and destructive ordering verified.');
}
