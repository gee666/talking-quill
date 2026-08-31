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

  it('authenticates the native medium controller and elevated worker', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('ShellExecuteExW');
    expect(setup).toContain('GetNamedPipeClientProcessId');
    expect(setup).toContain('GetNamedPipeServerProcessId');
    expect(setup).toContain('same-image worker');
    expect(setup).toContain('FILE_ATTRIBUTE_REPARSE_POINT');
    expect(setup).not.toMatch(/powershell|cmd.exe/iu);
  });

  it('reports installed and maintenance uninstall synchronously before deferred mapped-image cleanup', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('wait_relocated_status');
    expect(setup).toContain('uninstall-quarantined');
    expect(setup).toContain('arm_mapped_image_deletion');
    expect(setup).toContain(':tq-uninstall-');
    expect(setup).toContain('FILE_DISPOSITION_FLAG_POSIX_SEMANTICS');
    expect(setup).toContain('launch_same_token_uninstall_cleanup');
    expect(setup).not.toContain('let _ = elevate(&current, true');
    expect(setup).not.toContain('return Ok(if silent { ERROR_IO_PENDING');
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

  it('keeps durable install recovery inside the native worker', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    for (const phase of [
      'staging',
      'prepared',
      'committed',
      'uninstalling',
      'uninstall-quarantined',
    ]) {
      expect(setup).toContain(`"${phase}"`);
    }
    expect(setup).toContain('package::extract_file');
    expect(setup).toContain('remove_plain_tree');
  });
});
