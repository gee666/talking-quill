import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));

export function validateNsisUninstallPolicy({
  custom,
  assisted,
  uninstaller,
  installer,
  installSection,
  installUtil,
  installerInclude,
  common,
  extractAppPackage,
  oneInstance,
  multiUserUi,
  installValidation,
  cleanup,
  protectedBootstrap,
}) {
  if (custom.includes('!include "installer-publication.nsh"')) {
    throw new Error('Windows publication must not select the retired authority installer');
  }
  for (const forbidden of [
    'PersonalUserSid',
    'PersonalElevatedSid',
    'HKEY_USERS',
    'Get-Process explorer',
    'verify-legacy-program-files-auto',
    'backup-program-files-auto',
    'RestoreOldProgramFiles',
  ]) {
    if (custom.includes(forbidden)) {
      throw new Error(`personal installer still contains predecessor/user policy: ${forbidden}`);
    }
  }
  const isolatedFailureValues = [
    '/TALKINGQUILLTESTCOMMITFAIL=',
    'before-program-files-replace',
    'after-program-files-copy',
  ];
  if (
    isolatedFailureValues.some((value) => custom.includes(value)) ||
    !custom.includes('TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD') ||
    !custom.includes('installer-install-validation.nsh') ||
    !installValidation.includes('!macro TalkingQuillInitCommitFailurePoint') ||
    isolatedFailureValues.some((value) => !installValidation.includes(value))
  ) {
    throw new Error('install failure injection must exist only in the isolated validation include');
  }
  const install = macroBody(custom, 'customInstall');
  const launchGuard = install.indexOf('${IfNot} ${Silent}');
  const launch = install.indexOf('${StdUtils.ExecShellAsUser}');
  const launchGuardEnd = install.indexOf('${EndIf}', launch);
  if (
    install.includes('--enroll') ||
    install.includes('commit-install-auto') ||
    launchGuard < 0 ||
    launch <= launchGuard ||
    launchGuardEnd <= launch
  ) {
    throw new Error('personal install must launch without SCM enrollment');
  }
  if (!custom.includes('Function .onInstFailed') || !custom.includes('install-rollback')) {
    throw new Error('Windows migration must restore the protected predecessor on install failure');
  }
  for (const [sourceName, source] of [
    ['installer include', custom],
    ['isolated installer validation include', installValidation],
    ['electron-builder install utility template', installUtil],
    ['electron-builder package include', installerInclude],
    ['electron-builder common template', common],
    ['electron-builder installer template', installer],
    ['electron-builder uninstaller template', uninstaller],
    ['electron-builder extraction template', extractAppPackage],
    ['electron-builder running-process template', oneInstance],
    ['electron-builder assisted elevation template', multiUserUi],
  ]) {
    const unsafe = source
      .split(/\r?\n/u)
      .filter(
        (line) =>
          !/^\s*[;#]/u.test(line) &&
          /\bMessageBox\b/u.test(line) &&
          !/\/SD\s+ID(?:OK|NO|CANCEL|RETRY)\b/u.test(line),
      );
    if (unsafe.length !== 0) {
      throw new Error(`${sourceName} contains a MessageBox without a silent default`);
    }
  }
  const init = macroBody(custom, 'customInit');
  const installCommit = macroBody(custom, 'customInstallCommit');
  const lifecycleHook = macroBody(custom, 'customUninstallOldVersion');
  const rollbackFunction = functionBody(custom, 'TalkingQuillRollbackArmedInstall');
  const rollbackUntilRestored = functionBody(custom, 'TalkingQuillRollbackUntilRestored');
  const userAbort = functionBody(custom, 'TalkingQuillOnUserAbort');
  const installFailed = functionBody(custom, '.onInstFailed');
  const barrierCall = lifecycleHook.indexOf('TalkingQuillRunMachineCleanup install');
  const leaveInstallRoot = lifecycleHook.indexOf('SetOutPath "$TEMP"');
  const leaveInstallRootProcess = lifecycleHook.indexOf('SetCurrentDirectoryW', leaveInstallRoot);
  const armTransaction = lifecycleHook.indexOf('StrCpy $PersonalInstallTransactionArmed "1"');
  const cleanupFailureLabel = lifecycleHook.indexOf('personal_cleanup_failed:');
  const silentFailureGuard = lifecycleHook.indexOf('${IfNot} ${Silent}', cleanupFailureLabel);
  const cleanupFailureDialog = lifecycleHook.indexOf(
    'MessageBox MB_OK|MB_ICONSTOP',
    cleanupFailureLabel,
  );
  const commitCall = installCommit.indexOf('TalkingQuillRunMachineCleanup install-commit');
  const disarmTransaction = installCommit.indexOf('StrCpy $PersonalInstallTransactionArmed "0"');
  const retireBackup = installCommit.indexOf('TalkingQuillCleanCommittedRecoveryBestEffort');
  const applicationFiles = installSection.indexOf('!insertmacro installApplicationFiles');
  const commitHook = installSection.indexOf('!insertmacro customInstallCommit');
  const registryWrite = installSection.indexOf('!insertmacro registryAddInstallInfo');
  const shortcutWrite = installSection.indexOf('!insertmacro addStartMenuLink');
  if (
    barrierCall < 0 ||
    leaveInstallRoot < 0 ||
    leaveInstallRoot >= barrierCall ||
    leaveInstallRootProcess <= leaveInstallRoot ||
    leaveInstallRootProcess >= barrierCall ||
    armTransaction <= barrierCall ||
    cleanupFailureLabel <= armTransaction ||
    silentFailureGuard <= cleanupFailureLabel ||
    cleanupFailureDialog <= silentFailureGuard ||
    commitCall < 0 ||
    disarmTransaction <= commitCall ||
    retireBackup <= disarmTransaction ||
    applicationFiles < 0 ||
    commitHook <= applicationFiles ||
    registryWrite <= commitHook ||
    shortcutWrite <= registryWrite ||
    !custom.includes('TalkingQuillRunMachineCleanup install-retire-backup') ||
    !rollbackFunction.includes('$PersonalInstallRollbackStarted == "1"') ||
    !rollbackFunction.includes('TalkingQuillRunMachineCleanup install-rollback') ||
    !custom.includes('!define MUI_CUSTOMFUNCTION_ABORT TalkingQuillOnUserAbort') ||
    !rollbackUntilRestored.includes('Call TalkingQuillRollbackArmedInstall') ||
    !rollbackUntilRestored.includes('Goto rollback_until_restored') ||
    !userAbort.includes('Call TalkingQuillRollbackUntilRestored') ||
    !installFailed.includes('Call TalkingQuillRollbackUntilRestored')
  ) {
    throw new Error('install cancellation and failure must share guarded armed rollback');
  }
  const runningCheckGuard = installSection.indexOf('!ifmacrondef customUninstallOldVersion');
  const firstGeneratedRunningCheck = installSection.indexOf('!insertmacro CHECK_APP_RUNNING');
  const runningCheckGuardEnd = installSection.indexOf('!endif', firstGeneratedRunningCheck);
  const hookDefinition = installSection.indexOf('!ifmacrodef customUninstallOldVersion');
  const hookInvocation = installSection.indexOf('!insertmacro customUninstallOldVersion');
  const hookElse = installSection.indexOf('!else', hookInvocation);
  const generatedDiscovery = installSection.indexOf(
    '!insertmacro uninstallOldVersion SHELL_CONTEXT',
  );
  const hookEnd = installSection.indexOf('!endif', generatedDiscovery);
  const generatedSetOutPath = installSection.indexOf('SetOutPath $INSTDIR', hookEnd);
  const generatedInstallFiles = installSection.indexOf(
    '!insertmacro installApplicationFiles',
    generatedSetOutPath,
  );
  const installerSection = installer.indexOf('Section "install" INSTALL_SECTION_ID');
  const generatedSectionInclude = installer.indexOf('!include "installSection.nsh"');
  const generatedOnInit = installer.indexOf('Function .onInit');
  const earlyBootstrap = installer.indexOf('!insertmacro customEarlyInit', generatedOnInit);
  const firstOnInitWork = installer.indexOf('Call setInstallSectionSpaceRequired', generatedOnInit);
  const utilityQuoteGuard = installUtil.indexOf('!ifmacrondef customUninstallOldVersion');
  const utilityQuoteFunction = installUtil.indexOf('Function GetInQuotes');
  const utilityQuoteGuardEnd = installUtil.indexOf('!endif', utilityQuoteFunction);
  const utilityGuard = installUtil.indexOf(
    '!ifmacrondef customUninstallOldVersion',
    utilityQuoteGuard + 1,
  );
  const utilityFunction = installUtil.indexOf('Function uninstallOldVersion');
  const utilityMacro = installUtil.indexOf('!macro uninstallOldVersion ROOT_KEY');
  const utilityGuardEnd = installUtil.indexOf('!endif', utilityMacro);
  if (
    init.includes('TalkingQuillRunMachineCleanup install') ||
    !lifecycleHook.includes('TalkingQuillRunMachineCleanup install') ||
    runningCheckGuard < 0 ||
    firstGeneratedRunningCheck <= runningCheckGuard ||
    runningCheckGuardEnd <= firstGeneratedRunningCheck ||
    custom.includes('DeleteRegValue SHELL_CONTEXT "${UNINSTALL_REGISTRY_KEY}" "UninstallString"') ||
    custom.includes('PersonalPreviousUninstallString') ||
    hookDefinition < 0 ||
    hookInvocation <= hookDefinition ||
    hookElse <= hookInvocation ||
    generatedDiscovery <= hookElse ||
    hookEnd <= generatedDiscovery ||
    generatedSetOutPath <= hookEnd ||
    generatedInstallFiles <= generatedSetOutPath ||
    installerSection < 0 ||
    generatedSectionInclude <= installerSection ||
    generatedOnInit < 0 ||
    earlyBootstrap <= generatedOnInit ||
    firstOnInitWork <= earlyBootstrap ||
    installer.indexOf('!insertmacro customInit') < 0 ||
    installer.indexOf('!insertmacro customInit') >= installerSection ||
    utilityQuoteGuard < 0 ||
    utilityQuoteFunction <= utilityQuoteGuard ||
    utilityQuoteGuardEnd <= utilityQuoteFunction ||
    utilityGuard <= utilityQuoteGuardEnd ||
    utilityFunction <= utilityGuard ||
    utilityMacro <= utilityFunction ||
    utilityGuardEnd <= utilityMacro
  ) {
    throw new Error(
      'generated running-process and old-uninstaller paths must yield to the custom lifecycle barrier',
    );
  }
  const protectedBootstrapMacro = macroBody(custom, 'TalkingQuillProtectedEarlyBootstrap');
  const protectedPathValidation = protectedBootstrap.indexOf(
    '[Environment+SpecialFolder]::CommonApplicationData).TrimEnd',
  );
  const immutableTempCheck = protectedBootstrapMacro.indexOf('${If} $TEMP != $R1');
  const secureTempAssignment = protectedBootstrapMacro.indexOf(
    'StrCpy $TalkingQuillSecureTemp $R1',
  );
  const generatedUnOnInit = uninstaller.indexOf('Function un.onInit');
  const unEarlyBootstrap = uninstaller.indexOf('!insertmacro customUnEarlyInit', generatedUnOnInit);
  const firstUnOnInitWork = uninstaller.indexOf('SetOutPath $INSTDIR', generatedUnOnInit);
  if (
    generatedUnOnInit < 0 ||
    unEarlyBootstrap <= generatedUnOnInit ||
    firstUnOnInitWork <= unEarlyBootstrap ||
    !custom.includes('ExecShellWait "runas" "$EXEPATH"') ||
    !custom.includes('TALKING_QUILL_PERSONAL_INSTALLER') ||
    custom.indexOf('ExecShellWait "runas" "$EXEPATH"') > custom.indexOf('ExecWait') ||
    protectedBootstrapMacro.includes('StrCpy $TEMP') ||
    protectedPathValidation < 0 ||
    !protectedBootstrap.includes('NtQueryInformationProcess') ||
    !protectedBootstrap.includes('CommandLineToArgvW') ||
    !protectedBootstrap.includes('QueryFullProcessImageName') ||
    !protectedBootstrap.includes("StartsWith('/TQPROTECTEDTEMP='") ||
    !protectedBootstrap.includes('JoinArguments($childArguments)') ||
    !protectedBootstrap.includes('LastIndexOf(" _?=", StringComparison.Ordinal)') ||
    !protectedBootstrap.includes('[Microsoft.Win32.RegistryView]::Registry64') ||
    !protectedBootstrap.includes("Join-Path $nativeProgramFiles 'Talking Quill'") ||
    !protectedBootstrap.includes("$start.Arguments += ' ' + $nsisTail") ||
    !protectedBootstrap.includes('FileAttributes]::ReparsePoint') ||
    !protectedBootstrap.includes('SetAccessRuleProtection($true, $false)') ||
    !protectedBootstrap.includes("SetEnvironmentVariable('TEMP', $leaf, 'Process')") ||
    protectedBootstrap.indexOf("SetEnvironmentVariable('TEMP', $leaf, 'Process')") >
      protectedBootstrap.indexOf('Add-Type -TypeDefinition') ||
    /-Command[^\r\n]*\s"\$[R0-9]/u.test(protectedBootstrapMacro) ||
    immutableTempCheck < 0 ||
    secureTempAssignment <= immutableTempCheck
  ) {
    throw new Error('installer and uninstaller must elevate before protected plugin bootstrap');
  }
  const generatedUninstallRunningCheck = functionBody(uninstaller, 'un.checkAppRunning');
  const uninstallRunningGuard = generatedUninstallRunningCheck.indexOf(
    '!ifmacrondef customUnInstall',
  );
  const uninstallRunningCall = generatedUninstallRunningCheck.indexOf(
    '!insertmacro CHECK_APP_RUNNING',
  );
  if (
    uninstallRunningGuard < 0 ||
    uninstallRunningCall <= uninstallRunningGuard ||
    !oneInstance.includes('!ifmacrondef customUninstallOldVersion') ||
    !oneInstance.includes('!ifmacrondef customUnInstall') ||
    !cleanup.includes('function Normalize-RuntimeExecutablePath') ||
    !cleanup.includes("$Path.StartsWith('\\\\?\\', [StringComparison]::Ordinal)") ||
    !cleanup.includes('return $Path.Substring(4)') ||
    !cleanup.includes(
      '$path = Normalize-RuntimeExecutablePath -Path ([string]$_.ExecutablePath)',
    ) ||
    !cleanup.includes('function Assert-NoActiveRuntime') ||
    !cleanup.includes("$path -ieq (Join-Path $installRoot 'Talking Quill.exe')") ||
    /Stop-Process|taskkill/iu.test(
      cleanup.slice(
        cleanup.indexOf('function Assert-NoActiveRuntime'),
        cleanup.indexOf('function Remove-LegacyService'),
      ),
    )
  ) {
    throw new Error(
      'generated process killing must be unreachable before exact custom runtime checks',
    );
  }
  if (
    !common.includes('TALKING_QUILL_FORCE_UNSUPPORTED_ARCH_VALIDATION') ||
    !common.includes('"$(win7Required)" /SD IDOK') ||
    !common.includes('"$(x64WinRequired)" /SD IDOK') ||
    !extractAppPackage.includes('TALKING_QUILL_FORCE_EXTRACTION_FAILURE_VALIDATION') ||
    !extractAppPackage.includes('"$(decompressionFailed)$\\n$R0" /SD IDOK') ||
    !extractAppPackage.includes('!define ZIP_DECOMPRESSION_SUCCESS_LABEL') ||
    !extractAppPackage.includes('StrCmp $R0 "success" ${ZIP_DECOMPRESSION_SUCCESS_LABEL}') ||
    !extractAppPackage.includes('${ZIP_DECOMPRESSION_SUCCESS_LABEL}:') ||
    /StrCmp\s+\$R0\s+"success"\s+\+\d+/u.test(extractAppPackage) ||
    !extractAppPackage.includes('AbortExtract7za:\n    !ifmacrodef customInstallFailure') ||
    !extractAppPackage.includes('!insertmacro customInstallFailure') ||
    !custom.includes('!macro customInstallFailure') ||
    !installer.includes('"Unable to elevate, error $0" /SD IDOK')
  ) {
    throw new Error('silent-reachable generated failures must default safely and return nonzero');
  }
  macroBody(custom, 'customUnWelcomePage');
  const uninstall = macroBody(custom, 'customUnInstall');
  const remove = macroBody(custom, 'customRemoveFiles');
  const installCleanup = cleanup.slice(
    cleanup.indexOf("'install' {"),
    cleanup.indexOf("'install-rollback' {"),
  );
  const commitCleanup = cleanup.slice(
    cleanup.indexOf("'install-commit' {"),
    cleanup.indexOf("'install-retire-backup' {"),
  );
  const rollbackCleanup = cleanup.slice(
    cleanup.indexOf("'install-rollback' {"),
    cleanup.indexOf("'install-commit' {"),
  );
  const uninstallCleanup = cleanup.slice(cleanup.indexOf("'uninstall' {"));
  const backupCheck = uninstallCleanup.indexOf('Test-Path -LiteralPath $backupRoot');
  const installRootRemoval = uninstallCleanup.indexOf('Clear-OwnedTreeContents -Path $installRoot');
  if (
    !cleanup.includes("'.Talking Quill.stage1-transaction.json'") ||
    !cleanup.includes("'.Talking Quill.stage1-ambiguous-replacement'") ||
    !cleanup.includes('Preserve both candidates, restore the') ||
    !cleanup.includes('TalkingQuillAtomicFile') ||
    !cleanup.includes('MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH') ||
    !cleanup.includes("ValidateSet('recover', 'preserve-committed')") ||
    !cleanup.includes("$transaction.state -eq 'staging'") ||
    !cleanup.includes("$transaction.state -eq 'restoring'") ||
    !cleanup.includes("$transaction.state -eq 'committed'") ||
    !cleanup.includes('Write-InstallTransaction -State restoring') ||
    !cleanup.includes('after-first-restored-child') ||
    !cleanup.includes('recovery collision was preserved') ||
    !installCleanup.includes('Resolve-PendingInstallTransaction -CommittedAction recover') ||
    !installCleanup.includes('Write-InstallTransaction -State staging') ||
    !installCleanup.includes('Write-InstallTransaction -State prepared') ||
    !commitCleanup.includes('Write-InstallTransaction -State committed') ||
    !rollbackCleanup.includes(
      'Resolve-PendingInstallTransaction -CommittedAction preserve-committed',
    ) ||
    !uninstallCleanup.includes('Resolve-PendingInstallTransaction -CommittedAction recover')
  ) {
    throw new Error(
      'machine install recovery must use durable staging/prepared/restoring/committed transactions',
    );
  }
  if (
    /^Remove-LegacyService\s*$/mu.test(cleanup.slice(0, cleanup.indexOf('switch ($Mode)'))) ||
    /^\s*Remove-LegacyService\s*$/mu.test(installCleanup) ||
    !cleanup
      .slice(cleanup.indexOf("'install-retire-backup' {"), cleanup.indexOf("'uninstall' {"))
      .includes('Remove-LegacyService') ||
    !uninstallCleanup.includes('Remove-LegacyService')
  ) {
    throw new Error('legacy authority cleanup must occur only after replacement commit');
  }
  if (
    !custom.includes('${NSD_Uncheck} $DeleteTalkingQuillDataCheckbox') ||
    !custom.includes('MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2') ||
    !uninstall.includes('--talking-quill-reset-owned-data-and-exit') ||
    backupCheck < 0 ||
    installRootRemoval <= backupCheck ||
    !remove.includes('RMDir /r "$INSTDIR"') ||
    custom.includes('RMDir /r "$APPDATA') ||
    custom.includes('RMDir /r "$LOCALAPPDATA')
  ) {
    throw new Error('uninstall must preserve personal data unless confirmed');
  }
  if (
    assisted.indexOf('!ifmacrodef customUnWelcomePage') < 0 ||
    assisted.indexOf('!ifmacrodef customUnWelcomePage') >=
      assisted.indexOf('!insertmacro MUI_UNPAGE_INSTFILES')
  ) {
    throw new Error('data choice must appear before uninstall starts');
  }
  const installFiles = installSection.indexOf('!insertmacro installApplicationFiles');
  const customInstall = installSection.indexOf('!insertmacro customInstall');
  if (installFiles < 0 || customInstall < 0 || customInstall < installFiles) {
    throw new Error('migration commit must run after current files are installed');
  }
  const customUninstall = uninstaller.indexOf('!insertmacro customUnInstall');
  const installedDeletion = uninstaller.indexOf('# delete the installed files');
  const customRemoveFiles = uninstaller.indexOf('!insertmacro customRemoveFiles');
  if (
    customUninstall < 0 ||
    installedDeletion < 0 ||
    customRemoveFiles < 0 ||
    customUninstall >= installedDeletion ||
    customRemoveFiles <= installedDeletion
  ) {
    throw new Error('runtime cleanup and checked file removal hooks moved');
  }
}

function functionBody(source, name) {
  const start = source.search(new RegExp(`^Function ${escapeRegExp(name)}(?:\\s|$)`, 'mu'));
  const end = source.indexOf('FunctionEnd', start);
  if (start < 0 || end < 0) throw new Error(`Missing NSIS function ${name}`);
  return source.slice(start, end);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
}

function macroBody(source, name) {
  const start = source.search(new RegExp(`^!macro ${name}(?:\\s|$)`, 'mu'));
  const end = source.indexOf('!macroend', start);
  if (start < 0 || end < 0) throw new Error(`Missing NSIS macro ${name}`);
  return source.slice(start, end);
}

export async function loadNsisPolicyInputs() {
  const { createRequire } = await import('node:module');
  const require = createRequire(import.meta.url);
  const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
  const templateRoot = resolve(
    electronBuilderRequire.resolve('app-builder-lib/package.json'),
    '..',
    'templates',
    'nsis',
  );
  const read = (path) => readFile(path, 'utf8');
  return {
    custom: await read(resolve(repositoryRoot, 'build', 'installer.nsh')),
    assisted: await read(resolve(templateRoot, 'assistedInstaller.nsh')),
    uninstaller: await read(resolve(templateRoot, 'uninstaller.nsh')),
    installer: await read(resolve(templateRoot, 'installer.nsi')),
    installSection: await read(resolve(templateRoot, 'installSection.nsh')),
    installUtil: await read(resolve(templateRoot, 'include', 'installUtil.nsh')),
    installerInclude: await read(resolve(templateRoot, 'include', 'installer.nsh')),
    common: await read(resolve(templateRoot, 'common.nsh')),
    extractAppPackage: await read(resolve(templateRoot, 'include', 'extractAppPackage.nsh')),
    oneInstance: await read(resolve(templateRoot, 'include', 'allowOnlyOneInstallerInstance.nsh')),
    multiUserUi: await read(resolve(templateRoot, 'multiUserUi.nsh')),
    installValidation: await read(
      resolve(repositoryRoot, 'build', 'installer-install-validation.nsh'),
    ),
    cleanup: await read(resolve(repositoryRoot, 'build', 'windows-personal-machine-cleanup.ps1')),
    protectedBootstrap: await read(
      resolve(repositoryRoot, 'build', 'windows-protected-bootstrap.ps1'),
    ),
  };
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  validateNsisUninstallPolicy(await loadNsisPolicyInputs());
}
