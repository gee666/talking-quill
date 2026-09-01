import { Buffer } from 'node:buffer';
import { readFile } from 'node:fs/promises';
import { describe, expect, it, vi } from 'vitest';
import {
  buildWindowsElevationLaunch,
  settleWindowsElevation,
} from '../../app/src/main/info/windows-update-launch';
import {
  readWindowsUpdateRelaunchGeneration,
  wrapWindowsUpdateRelaunchRequest,
} from '../../app/src/main/info/windows-update-relaunch-intent';

const candidate = {
  version: '0.0.69',
  platform: 'win' as const,
  architecture: 'x64' as const,
  ownerMode: 'local-unsigned-enabled' as const,
  packageMode: 'update' as const,
  sourceCommit: '88'.repeat(20),
  sourceTree: '99'.repeat(20),
  releaseBuildDigest: '11'.repeat(32),
  packageLayoutDigest: '22'.repeat(32),
  packageSha256: 'ab'.repeat(32),
  channel: 'latest-x64',
  transactionBinding: 'source-target-package-sha256-v1' as const,
  authorization: {
    scheme: 'p256-sha256-v1' as const,
    verificationKeySha256: '66'.repeat(32),
    signature: 'MEUCIQfixture==',
  },
  roles: [
    {
      role: 'gateway',
      path: 'resources/helper/talking-quill-helper.exe',
      sha256: '33'.repeat(32),
      suppressionCapable: false,
    },
    {
      role: 'owner',
      path: 'resources/helper/talking-quill-keyboard-owner.exe',
      sha256: '44'.repeat(32),
      suppressionCapable: true,
    },
  ],
  predecessor: {
    platform: 'win' as const,
    architecture: 'x64' as const,
    version: '0.0.67',
    releaseBuildDigest: '55'.repeat(32),
    gatewaySha256: '66'.repeat(32),
    ownerSha256: '77'.repeat(32),
  },
};

describe('Windows elevated updater launch', () => {
  it('accepts one exact relaunch generation and rejects ambiguous command lines', () => {
    const argument = `--windows-update-relaunch-generation-v1=${'ab'.repeat(16)}`;
    expect(readWindowsUpdateRelaunchGeneration(['Talking Quill.exe', argument])).toBe(
      'ab'.repeat(16),
    );
    expect(readWindowsUpdateRelaunchGeneration(['Talking Quill.exe'])).toBeNull();
    expect(readWindowsUpdateRelaunchGeneration([argument, argument])).toBeNull();
    expect(
      readWindowsUpdateRelaunchGeneration([
        '--windows-update-relaunch-generation-v1=not-a-generation',
      ]),
    ).toBeNull();
  });

  it('takes verified predecessor locks in fixed order before the protected file lock', async () => {
    const [helper, setup] = await Promise.all([
      readFile('helper/src/windows_update.rs', 'utf8'),
      readFile('installer/windows-setup/src/windows.rs', 'utf8'),
    ]);
    for (const source of [helper, setup]) {
      expect(source).toContain('RecoveryStateLockV1');
      expect(source).toContain('DirectorySuffix');
      expect(source).toContain('recovery-state-v1.lock');
      expect(source).toContain('file_identity_text');
      expect(source).toContain('.share_mode(0)');
      expect(source).toContain('MACHINE_LOCK_PENDING_PREFIX');
      expect(source).toContain('publication-pending-v1');
      expect(source).toContain('LEGACY_LOCK_RETIREMENT_EPOCH');
      expect(source.indexOf(String.raw`Global\\TalkingQuill.NativeSetup.V2`)).toBeLessThan(
        source.indexOf(String.raw`Global\\TalkingQuill.UpdateRecovery.State.V1`),
      );
    }
  });

  it('hands terminal cleanup to an authenticated transient native service', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('uninstall-finalizer-publishing');
    expect(setup).toContain('uninstall-app-path-retiring');
    expect(setup).toContain('uninstall-app-path-retired');
    expect(setup).toContain('uninstall-registration-retiring');
    expect(setup).toContain('uninstall-finalizer-deletion-owned');
    expect(setup).toContain('TQ-KEEP-IMAGE');
    expect(setup).toContain('FILE_DISPOSITION_FLAG_POSIX_SEMANTICS');
    expect(setup).toContain('MOVEFILE_DELAY_UNTIL_REBOOT');
    expect(setup).toContain('PendingFileRenameOperations');
    expect(setup).toContain('publish_terminal_uninstall_record(paths, current)?');
    expect(setup).toContain('StartServiceCtrlDispatcherW');
    expect(setup).toContain('CreateServiceW');
    expect(setup).toContain('SERVICE_AUTO_START');
    expect(setup).toContain('SERVICE_WIN32_OWN_PROCESS');
    expect(setup).toContain('/TQ-TERMINAL-SERVICE={generation}');
    expect(setup).toContain('schedule_terminal_service_deletion(current)?');
    expect(setup).not.toContain('schedule_delayed_deletion_plan');
    expect(setup).toContain('fn relocated_uninstall_matches_maintenance');
    expect(setup).toContain('authenticated_relocated_image');
    const finalize = setup.slice(setup.indexOf('fn finalize_uninstall'));
    expect(finalize.indexOf('system.unregister_app_path()?')).toBeLessThan(
      finalize.indexOf('system.unregister_uninstall()?'),
    );
    expect(finalize.indexOf('publish_terminal_uninstall_record(paths, current)?')).toBeLessThan(
      finalize.indexOf('remove_transaction(paths)?'),
    );
    expect(finalize.indexOf('remove_transaction(paths)?')).toBeLessThan(
      finalize.indexOf('system.unregister_uninstall()?'),
    );
    const retirement = setup.slice(
      setup.indexOf('fn retire_terminal_machine_state'),
      setup.indexOf('enum RecoveryPlan'),
    );
    expect(retirement.indexOf('recover_with_adapter(paths, system)?')).toBeLessThan(
      retirement.indexOf('system.unregister_app_path()?'),
    );
    expect(retirement.indexOf('remove_transaction(paths)?')).toBeLessThan(
      retirement.indexOf('write_terminal_uninstall_phase(paths, generation, "machine-retired")'),
    );
    expect(
      retirement.indexOf('write_terminal_uninstall_phase(paths, generation, "machine-retired")'),
    ).toBeLessThan(retirement.lastIndexOf('system.unregister_uninstall()'));
    expect(retirement).toContain('register_uninstall_executable(&paths.maintenance_uninstaller)?');
    expect(setup).toContain(
      'RegCreateKeyExW(\n            HKEY_LOCAL_MACHINE,\n            wide(OsStr::new(UNINSTALL_KEY))',
    );
    const terminalCleanup = setup.slice(
      setup.indexOf('fn finish_terminal_uninstall'),
      setup.indexOf('fn clear_machine_relaunch_owner'),
    );
    expect(terminalCleanup.indexOf('clear_legacy_profile_relaunch_owners(paths)?')).toBeLessThan(
      terminalCleanup.lastIndexOf('clear_machine_relaunch_owner(paths)'),
    );
    expect(terminalCleanup.indexOf('clear_machine_relaunch_owner(paths)?')).toBeLessThan(
      terminalCleanup.indexOf('arm_mapped_image_deletion(&launcher)?'),
    );
    expect(setup).toContain('DeleteService(service.0)');
    expect(setup).toContain('arm_mapped_image_deletion(current)?');
    expect(setup).not.toContain('TERMINAL_RUN_ONCE_PREFIX');
    expect(setup).not.toContain('CurrentVersion\\RunOnce');
    expect(setup).toContain('decode_pending_rename_pairs');
    expect(setup).toContain('destination.is_empty()');
    const lockRetirement = setup.slice(
      setup.indexOf('fn retire_machine_lock_publication'),
      setup.indexOf('fn reclaim_unpublished_machine_lock_directories'),
    );
    expect(lockRetirement).toContain('delete_registry_tree_durable(');
    const terminalRecovery = setup.slice(
      setup.indexOf('fn run_terminal_cleanup_service'),
      setup.indexOf('fn enumerate_registry_subkeys'),
    );
    expect(terminalRecovery.indexOf('retire_machine_lock_publication(&paths)?')).toBeLessThan(
      terminalRecovery.indexOf('remove_machine_lock_residue(&paths, &suffix)?'),
    );
    const residue = setup.slice(setup.indexOf('if uninstall_authorized'));
    expect(residue).toContain('read_terminal_uninstall_record(&paths)?');
  });

  it('persists a nonce-bound native relaunch wrapper before elevation', async () => {
    const [main, helper, application, setup] = await Promise.all([
      readFile('helper/src/main.rs', 'utf8'),
      readFile('helper/src/windows_update.rs', 'utf8'),
      readFile('app/src/main/app/application.ts', 'utf8'),
      readFile('installer/windows-setup/src/windows.rs', 'utf8'),
    ]);
    expect(main).toContain('--windows-update-bootstrap-v3=');
    expect(main).toContain('--windows-update-app-ready-v1=');
    expect(main).toContain('--windows-update-relaunch-owner-install-v1');
    expect(helper).toContain('RELAUNCH_RUN_VALUE');
    expect(helper).toContain('--windows-update-relaunch-owner-v1');
    expect(helper).toContain('HKEY_LOCAL_MACHINE');
    expect(helper).toContain('current_relaunch_identity()');
    expect(helper).toContain('FOLDERID_ProgramData');
    expect(helper).toContain(
      'launch_elevated_installed_helper("--windows-update-relaunch-owner-install-v1")',
    );
    expect(helper).toContain('command.encode_utf16().count() > 260');
    expect(helper).not.toContain('--windows-update-bootstrap-v3={encoded}');
    expect(helper).toContain('relaunch-record-marker-v1');
    expect(helper).toContain('MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH');
    expect(helper).toContain('read_persisted_relaunch_record(&request.generation)?');
    expect(setup).toContain('protected launcher is a stable machine component');
    expect(helper).toContain('"setup-complete"');
    expect(helper).toContain('"launch-started"');
    expect(helper).toContain('defer_launch_until_parent_exit');
    expect(helper).toContain('verified_surviving_version');
    expect(helper).not.toContain('for recovery_generation in owned_recovery_generations()?');
    expect(helper).toContain('record.recovery_generation');
    expect(helper).toContain('schema_version: 3');
    expect(helper).toContain('record.schema_version != 3');
    expect(helper).toContain('RELEASED_NO_RECORD_PREDECESSOR: &str = "0.0.67"');
    expect(helper).not.toContain('migrate_legacy_relaunch');
    expect(helper).toContain('validate_acquired_machine_lock_state(&path)?');
    expect(helper).toContain('terminal_uninstall_record()?');
    expect(setup).toContain('RegLoadAppKeyW');
    expect(setup).toContain('clear_legacy_relaunch_values_in_hive');
    expect(helper).toContain('verify_app_ready_parent');
    const running = application.indexOf("this.#lifecycle = 'running'");
    expect(running).toBeLessThan(
      application.indexOf('this.#acknowledgePendingWindowsUpdateRelaunches();', running),
    );
    const request = '--windows-update-bootstrap-v2=YWJjZA==';
    const wrapped = wrapWindowsUpdateRelaunchRequest(
      request,
      'C:\\Users\\Test\\AppData\\Roaming\\Talking Quill\\windows-update-relaunch-intent-v1.json',
      '11'.repeat(16),
    );
    const payload = JSON.parse(
      Buffer.from(wrapped.split('=').slice(1).join('='), 'base64').toString('utf8'),
    ) as Record<string, unknown>;
    expect(payload).toEqual({
      request,
      intentPath:
        'C:\\Users\\Test\\AppData\\Roaming\\Talking Quill\\windows-update-relaunch-intent-v1.json',
      nonce: '11'.repeat(16),
    });
  });

  it('publishes protected marker files through closed handles and verifies identity after rename', async () => {
    const [helper, setup] = await Promise.all([
      readFile('helper/src/windows_update.rs', 'utf8'),
      readFile('installer/windows-setup/src/windows.rs', 'utf8'),
    ]);
    for (const source of [helper, setup]) {
      expect(source).toContain('.tmp-{}');
      expect(source).toContain('MOVEFILE_WRITE_THROUGH');
      expect(source).toContain('sync_all()');
      expect(source).toContain('drop(file)');
      expect(source).toContain('Some(&identity)');
      expect(source).toContain('.share_mode(0)');
    }
  });

  it('elevates the installed bootstrap with an opaque exact package request', () => {
    const hash = 'ab'.repeat(32);
    const launch = buildWindowsElevationLaunch(
      'C:\\Windows',
      'C:\\Program Files\\Talking Quill\\resources\\helper\\talking-quill-helper.exe',
      'C:\\Updates\\Talking Quill.exe',
      hash,
      candidate,
    );
    expect(launch.executable).toBe(
      'C:\\Program Files\\Talking Quill\\resources\\helper\\talking-quill-helper.exe',
    );
    expect(launch.arguments).toHaveLength(1);
    const argument = launch.arguments[0] ?? '';
    expect(argument).toMatch(/^--windows-update-bootstrap-v2=/u);
    expect(
      JSON.parse(
        Buffer.from(argument.split('=', 2)[1] ?? '', 'base64').toString('utf8'),
      ) as unknown,
    ).toEqual({
      version: 2,
      installerPath: 'C:\\Updates\\Talking Quill.exe',
      sha256: hash,
      candidate,
    });
  });

  it('reopens and verifies the installer in the elevated installed bootstrap before resume', async () => {
    const [bootstrap, main] = await Promise.all([
      readFile('helper/src/windows_update.rs', 'utf8'),
      readFile('helper/src/main.rs', 'utf8'),
    ]);
    expect(main).toContain('--windows-update-bootstrap-v2=');
    expect(main).toContain('--windows-update-bootstrap-staged-v2=');
    expect(main).not.toContain('--windows-update-bootstrap-staged-v1=');
    expect(bootstrap).toContain('TokenElevation');
    expect(bootstrap).toContain('trusted_installed_bootstrap()');
    expect(bootstrap).toContain('FOLDERID_ProgramFiles');
    expect(bootstrap).toContain('verify_update_relation(&request.candidate, &request.sha256)');
    expect(bootstrap).toContain('verify_update_authorization(&request.candidate)');
    expect(bootstrap).toContain('parse_and_authorize_request(suffix)');
    expect(bootstrap).toContain('env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1")');
    expect(bootstrap).not.toContain('option_env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1")');
    expect(bootstrap).not.toContain('TALKING_QUILL_WINDOWS_UPDATE_BRIDGE_KEY_V1=');
    expect(bootstrap).not.toContain('TALKING_QUILL_WINDOWS_UPDATE_BRIDGE_PUBLIC_KEY_SEC1');
    expect(bootstrap).not.toContain('running_from_candidate_gateway');
    const authorize = bootstrap.indexOf('let request = parse_and_authorize_request(suffix)');
    const hashInstaller = bootstrap.indexOf('hash_file(&mut installer)', authorize);
    const resumeStaged = bootstrap.indexOf(
      'ResumeThread(thread_handle.as_raw_handle())',
      authorize,
    );
    expect(hashInstaller).toBeGreaterThan(authorize);
    expect(resumeStaged).toBeGreaterThan(hashInstaller);
    expect(bootstrap).toContain('candidate.predecessor.gateway_sha256');
    expect(bootstrap).toContain('RESTRICTED_STAGING_SDDL');
    expect(bootstrap).toContain('RECOVERY_LAUNCHER_PENDING_PREFIX');
    expect(bootstrap).toContain('publish_medium_launcher_directory');
    expect(bootstrap).toContain('reclaim_incomplete_launcher_directories');
    expect(bootstrap).toContain('RECOVERY_LAUNCHER_IDENTITY_NAME');
    expect(bootstrap).toContain('D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)');
    expect(bootstrap).toContain('CreateDirectoryW(path.as_ptr(), &attributes)');
    expect(bootstrap).toContain('PROTECTED_DACL_SECURITY_INFORMATION');
    expect(bootstrap).toContain('apply_restricted_dacl(&pending_staged, RESTRICTED_FILE_SDDL)');
    expect(bootstrap).toContain('apply_restricted_dacl(&target, RESTRICTED_FILE_SDDL)');
    expect(bootstrap).toContain('.share_mode(FILE_SHARE_READ)');
    expect(bootstrap).toContain('CREATE_SUSPENDED');
    expect(bootstrap).toContain('verify_suspended_process');
    expect(bootstrap).toContain('GetFileInformationByHandle');
    expect(bootstrap).toContain('QueryFullProcessImageNameW');
    expect(bootstrap.indexOf('trusted_installed_bootstrap()?')).toBeLessThan(
      bootstrap.indexOf('create_restricted_directory(&pending)?'),
    );
    expect(bootstrap).toContain('file_identity(&trusted)');
    expect(bootstrap).toContain('hash_file(&mut trusted)');
    expect(bootstrap.indexOf('verify_suspended_process')).toBeLessThan(
      bootstrap.lastIndexOf('ResumeThread'),
    );
    expect(bootstrap).not.toContain(
      'WaitForSingleObject(process_handle.as_raw_handle(), u32::MAX)',
    );
    expect(bootstrap).toContain('INSTALLER_SUPERVISION_TIMEOUT_MS');
    expect(bootstrap).toContain('EXIT_INSTALLER_STILL_RUNNING');
    expect(bootstrap).toContain('if result == Ok(0)');
    expect(bootstrap).toContain('schedule_staged_cleanup(Some(generation))?;');
    expect(bootstrap).toContain('retain the same generation and counter');
    expect(bootstrap).toContain('GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0');
    expect(bootstrap).toContain('return Err(EXIT_LAUNCH_FAILED);');
    expect(bootstrap).toContain('if wait == WAIT_TIMEOUT');
  });

  it('acknowledges bootstrap elevation without forcing Electron to quit', () => {
    const accepted = vi.fn();
    const cancelled = vi.fn();
    const failed = vi.fn();
    settleWindowsElevation(0, accepted, cancelled, failed);
    expect(accepted).toHaveBeenCalledOnce();
    expect(cancelled).not.toHaveBeenCalled();
    expect(failed).not.toHaveBeenCalled();
  });

  it('distinguishes confirmed UAC cancellation from recoverable native failures', () => {
    const accepted = vi.fn();
    const cancelled = vi.fn();
    const failed = vi.fn();
    settleWindowsElevation(1223, accepted, cancelled, failed);
    expect(cancelled).toHaveBeenCalledOnce();
    settleWindowsElevation(70, accepted, cancelled, failed);
    expect(failed).toHaveBeenCalledOnce();
    expect(accepted).not.toHaveBeenCalled();
  });

  it.each([
    { ...candidate, channel: 'latest-arm64' },
    { ...candidate, architecture: 'arm64' as const },
    { ...candidate, roles: candidate.roles.slice().reverse() },
    {
      ...candidate,
      roles: candidate.roles.map((role, index) =>
        index === 1 ? { ...role, sha256: '0'.repeat(63) } : role,
      ),
    },
    {
      ...candidate,
      predecessor: { ...candidate.predecessor, gatewaySha256: 'ff'.repeat(31) },
    },
    { ...candidate, sourceCommit: '0'.repeat(39) },
    { ...candidate, sourceTree: 'A'.repeat(40) },
  ])('rejects a hostile candidate identity %# before invoking the native bootstrap', (changed) => {
    expect(() =>
      buildWindowsElevationLaunch(
        'C:\\Windows',
        'C:\\Program Files\\Talking Quill\\resources\\helper\\talking-quill-helper.exe',
        'C:\\Updates\\Talking Quill.exe',
        'ab'.repeat(32),
        changed,
      ),
    ).toThrow('Invalid downloaded Windows installer identity');
  });

  it('allows an authenticated same-version refresh when the release build differs', () => {
    expect(() =>
      buildWindowsElevationLaunch(
        'C:\\Windows',
        'C:\\Program Files\\Talking Quill\\resources\\helper\\talking-quill-helper.exe',
        'C:\\Updates\\Talking Quill.exe',
        candidate.packageSha256,
        {
          ...candidate,
          predecessor: { ...candidate.predecessor, version: candidate.version },
        },
      ),
    ).not.toThrow();
  });

  it('rejects malformed installer digests before invoking PowerShell', () => {
    expect(() =>
      buildWindowsElevationLaunch(
        'C:\\Windows',
        'C:\\Program Files\\Talking Quill\\resources\\helper\\talking-quill-helper.exe',
        'candidate.exe',
        '0'.repeat(63),
        candidate,
      ),
    ).toThrow('Invalid downloaded Windows installer identity');
  });
});
