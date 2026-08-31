import { Buffer } from 'node:buffer';
import { readFile } from 'node:fs/promises';
import { describe, expect, it, vi } from 'vitest';
import {
  buildWindowsElevationLaunch,
  settleWindowsElevation,
} from '../../app/src/main/info/windows-update-launch';
import { wrapWindowsUpdateRelaunchRequest } from '../../app/src/main/info/windows-update-relaunch-intent';

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
  it('takes verified legacy migration locks in fixed order before the protected file lock', async () => {
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

  it('keeps a callable finalizer through terminal commit and uses native POSIX self-removal', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('uninstall-finalizer-publishing');
    expect(setup).toContain('uninstall-app-path-retiring');
    expect(setup).toContain('uninstall-app-path-retired');
    expect(setup).toContain('uninstall-registration-retiring');
    expect(setup).toContain('uninstall-registration-retired');
    expect(setup).toContain('TQ-KEEP-IMAGE');
    expect(setup).toContain('FILE_DISPOSITION_FLAG_POSIX_SEMANTICS');
    expect(setup).not.toContain('MOVEFILE_DELAY_UNTIL_REBOOT');
    const finalize = setup.slice(setup.indexOf('fn finalize_uninstall'));
    expect(finalize.indexOf('system.unregister_app_path()?')).toBeLessThan(
      finalize.indexOf('system.unregister_uninstall()?'),
    );
    expect(finalize.indexOf('remove_transaction(paths)?')).toBeLessThan(
      finalize.indexOf('system.unregister_uninstall()?'),
    );
  });

  it('persists a nonce-bound native relaunch wrapper before elevation', () => {
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
