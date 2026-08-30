import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('Windows gateway and owner lifecycle contract', () => {
  it('keeps the native package to one gateway and one detached owner', async () => {
    const [contract, launcher, windowsRuntime, runtime] = await Promise.all([
      readFile('scripts/helper-build-contract.mjs', 'utf8'),
      readFile('helper/src/owner/windows.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/windows_runtime.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/runtime.rs', 'utf8'),
    ]);
    expect(contract).toContain(
      "CANONICAL_WINDOWS_NATIVE_ROLES = Object.freeze(['gateway', 'owner'])",
    );
    expect(contract).toContain("name: 'talking-quill-helper.exe'");
    expect(contract).toContain("name: 'talking-quill-keyboard-owner.exe'");
    expect(contract).not.toContain('TalkingQuillKeyboardAuthority.exe');
    expect(launcher).toContain('current_pipe_name');
    expect(launcher).toContain('named_pipe_server_pid');
    expect(launcher).not.toContain('PROC_THREAD_ATTRIBUTE_HANDLE_LIST');
    expect(launcher).not.toContain('TerminateProcess');
    expect(windowsRuntime).toContain('Local\\\\TalkingQuill.KeyboardOwner.Personal.V1.{session}');
    expect(runtime).toContain('bind_owner_instance(owner_instance)');
    expect(runtime.indexOf('bind_owner_instance(owner_instance)')).toBeLessThan(
      runtime.indexOf('PlatformAdapter::start()'),
    );
    expect(runtime).toContain('native_startup_failed || shutdown.is_err()');
    expect(runtime).toContain('singleton.preserve_process_lifetime()');
  });

  it('drives application readiness, gateway crash, idle owner process drain, and clean relaunch', async () => {
    const driver = await readFile('scripts/windows-package-lifecycle.mjs', 'utf8');
    expect(driver).toContain("await readinessLaunch('initial')");
    expect(driver).toContain('talking-quill-application-running');
    expect(driver).toContain('main-capture-widget-created');
    expect(driver).toContain('persisted-profile-activation');
    expect(driver).toContain('persistedSettings.dictationProfiles');
    expect(driver).toContain('--talking-quill-installed-lifecycle-user-data=');
    expect(driver).toContain("first.userDataRootSha256 !== createHash('sha256').update(profile)");
    expect(driver).toContain('process.kill(before.gateways[0].ProcessId)');
    expect(driver).toContain('await waitForRolesAbsent(45_000)');
    expect(driver).toContain("$path.StartsWith('\\\\\\\\?\\\\')");
    expect(driver).toContain('$path=$path.Substring(4)');
    expect(driver).toContain("$prefix=$root+'\\\\'");
    expect(driver).toContain('$path.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)');
    expect(driver).toContain('root.toLowerCase() !== expectedInstalledRoot.toLowerCase()');
    expect(driver).not.toContain(
      '!root.toLowerCase().startsWith(String(process.env.ProgramFiles).toLowerCase())',
    );
    expect(driver).toContain("await readinessLaunch('successor')");
    expect(driver).toContain("mode === 'installed'");
    expect(driver).toContain("status: 'unavailable'");
    expect(driver).toContain('authoritative: false');
    expect(driver).toContain("exercise: 'idle-gateway-crash-and-sequential-relaunch'");
    expect(driver).toContain('does not exercise held-key ownership');
    expect(driver).toContain('physical keyboard suppression');
    expect(driver).not.toMatch(/\bSendInput\s*\(/u);
    expect(driver).not.toContain('evidence');
  });

  it('relaunches into validated ProgramData before any privileged plugin or cleanup code', async () => {
    const [installer, patch, lifecycle] = await Promise.all([
      readFile('build/installer.nsh', 'utf8'),
      readFile('patches/app-builder-lib@26.15.3.patch', 'utf8'),
      readFile('helper/src/windows_installer.rs', 'utf8'),
    ]);
    expect(patch).toContain('!insertmacro customEarlyInit');
    expect(installer).toContain('!macro customEarlyInit');
    expect(installer).toContain('[Environment+SpecialFolder]::CommonApplicationData');
    expect(installer).toContain('SetAccessRuleProtection($$true,$$false)');
    expect(installer).toContain('/TQPROTECTEDTEMP=');
    expect(installer).toContain('AreAccessRulesProtected');
    expect(installer).toContain('ReparsePoint');
    expect(installer).toContain('talking-quill-installer-lifecycle.exe');
    expect(installer).not.toContain('File /oname=$PLUGINSDIR\\talking-quill-machine-cleanup.ps1');
    expect(lifecycle).toContain('FOLDERID_ProgramFiles');
    expect(lifecycle).toContain('verify_candidate');
    expect(lifecycle).toContain('fresh-install');
  });

  it('has no superseded Windows owner transport or lifecycle seams', async () => {
    const [gateway, ownerRuntime, ownerConnection, diagnostics] = await Promise.all([
      readFile('helper/src/owner/windows.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/runtime.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/windows_connection.rs', 'utf8'),
      readFile('docs/owner-disconnect-diagnostic-contract.md', 'utf8'),
    ]);
    const source = `${gateway}\n${ownerRuntime}\n${ownerConnection}`;
    for (const obsolete of [
      'GatewayLaunchElection',
      'PROC_THREAD_ATTRIBUTE_HANDLE_LIST',
      'TALKING_QUILL_TEST_OLD_ORPHAN_BEHAVIOR',
      'WindowsTicketP256',
      'issue_ticket',
    ]) {
      expect(source).not.toContain(obsolete);
    }
    expect(diagnostics).toContain('cannot close the callback gate');
    expect(diagnostics).toContain(
      'never becomes capture, reconnect, shutdown, or process-lifecycle authority',
    );
  });

  it('uses the elevated installer only for rollback-safe files and retired cleanup', async () => {
    const cleanup = await readFile('build/windows-personal-machine-cleanup.ps1', 'utf8');
    expect(cleanup).toContain("$serviceName = 'TalkingQuillKeyboardAuthority'");
    expect(cleanup).toContain("'Talking Quill\\KeyboardAuthority'");
    expect(cleanup).toContain('[Microsoft.Win32.RegistryView]::Registry64');
    expect(cleanup).toContain("GetValue('ProgramFilesDir'");
    expect(cleanup).toContain('$machineRoot = Get-NativeProgramFiles');
    expect(cleanup).not.toContain(
      '[Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)',
    );
    expect(cleanup).toContain('Assert-PlainOwnedTree');
    expect(cleanup).toContain('function Normalize-RuntimeExecutablePath');
    expect(cleanup).toContain("$Path.StartsWith('\\\\?\\', [StringComparison]::Ordinal)");
    expect(cleanup).toContain('return $Path.Substring(4)');
    expect(cleanup).toContain(
      '$path = Normalize-RuntimeExecutablePath -Path ([string]$_.ExecutablePath)',
    );
    expect(cleanup).toContain('Request-PlannedRuntimeExit');
    expect(cleanup).toContain('--talking-quill-request-machine-quit');
    expect(cleanup).toContain('did not confirm neutral planned exit');
    expect(cleanup).toContain('Write-InstallTransaction -State staging');
    expect(cleanup).toContain("$transaction.state -eq 'staging'");
    expect(cleanup).toContain('recovery collision was preserved');
    expect(cleanup).toContain(
      'Move-OwnedTreeContents -Source $installRoot -Destination $backupRoot',
    );
    const commit = cleanup.slice(
      cleanup.indexOf("'install-commit' {"),
      cleanup.indexOf("'install-retire-backup' {"),
    );
    expect(commit).not.toContain('Remove-Item -LiteralPath $backupRoot');
    const retirement = cleanup.slice(
      cleanup.indexOf("'install-retire-backup' {"),
      cleanup.indexOf("'uninstall' {"),
    );
    expect(retirement).toContain('Remove-LegacyService');
    expect(retirement).toContain('Resolve-PendingInstallTransaction -CommittedAction recover');
    expect(cleanup).toMatch(
      /\$transaction\.state -eq 'committed'[\s\S]*Remove-Item -LiteralPath \$backupRoot/u,
    );
    const installPreparation = cleanup.slice(
      cleanup.indexOf("'install' {"),
      cleanup.indexOf("'install-rollback' {"),
    );
    expect(installPreparation).not.toContain('Remove-LegacyService');
    const uninstall = cleanup.slice(cleanup.indexOf("'uninstall' {"));
    expect(uninstall).toContain('Remove-LegacyService');
    expect(uninstall.indexOf('Test-Path -LiteralPath $backupRoot')).toBeLessThan(
      uninstall.indexOf('Clear-OwnedTreeContents -Path $installRoot'),
    );
    expect(cleanup).not.toContain('--enroll');
    expect(cleanup).not.toContain('CreateService');
  });
});
