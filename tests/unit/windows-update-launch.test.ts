import { Buffer } from 'node:buffer';
import { readFile } from 'node:fs/promises';
import { describe, expect, it, vi } from 'vitest';
import {
  buildWindowsElevationLaunch,
  settleWindowsElevation,
} from '../../app/src/main/info/windows-update-launch';

const candidate = {
  version: '0.0.68',
  platform: 'win' as const,
  architecture: 'x64' as const,
  ownerMode: 'local-unsigned-enabled' as const,
  packageMode: 'update' as const,
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
      'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe',
    );
    expect(launch.arguments).toEqual([
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      expect.stringContaining('-Verb RunAs -PassThru -ErrorAction Stop'),
    ]);
    const script = launch.arguments.at(-1) ?? '';
    expect(script).not.toContain('C:\\Updates\\Talking Quill.exe');
    expect(script).not.toContain('C:\\Program Files\\Talking Quill');
    expect(script).not.toContain(hash);
    expect(script).toContain('$bootstrap=');
    expect(script).toContain('$argument=');
    expect(launch.arguments).not.toContain('-File');
    expect(script).not.toMatch(/Set-Content|Out-File|WriteAllText|\.ps1/iu);

    const encoded = /\$argument=.*FromBase64String\('([^']+)'\)/u.exec(script)?.[1];
    expect(encoded).toBeDefined();
    const argument = Buffer.from(encoded ?? '', 'base64').toString('utf16le');
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
    expect(bootstrap).toContain('D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)');
    expect(bootstrap).toContain('CreateDirectoryW(path.as_ptr(), &attributes)');
    expect(bootstrap).toContain('PROTECTED_DACL_SECURITY_INFORMATION');
    expect(bootstrap).toContain('apply_restricted_dacl(&staged, RESTRICTED_FILE_SDDL)');
    expect(bootstrap).toContain('apply_restricted_dacl(&target, RESTRICTED_FILE_SDDL)');
    expect(bootstrap).toContain('.share_mode(FILE_SHARE_READ)');
    expect(bootstrap).toContain('CREATE_SUSPENDED');
    expect(bootstrap).toContain('verify_suspended_process');
    expect(bootstrap).toContain('GetFileInformationByHandle');
    expect(bootstrap).toContain('QueryFullProcessImageNameW');
    expect(bootstrap.indexOf('trusted_installed_bootstrap()?')).toBeLessThan(
      bootstrap.indexOf('create_restricted_directory(&directory)?'),
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
    expect(bootstrap).toContain('if result != Err(EXIT_INSTALLER_STILL_RUNNING)');
    expect(bootstrap).toContain('if wait == WAIT_TIMEOUT');
  });

  it('acknowledges bootstrap elevation without forcing Electron to quit', () => {
    const accepted = vi.fn();
    const rejected = vi.fn();
    settleWindowsElevation(0, accepted, rejected);
    expect(accepted).toHaveBeenCalledOnce();
    expect(rejected).not.toHaveBeenCalled();
  });

  it.each([1, 3, null])('keeps the application alive for UAC cancellation/failure (%s)', (code) => {
    const accepted = vi.fn();
    const rejected = vi.fn();
    settleWindowsElevation(code, accepted, rejected);
    expect(accepted).not.toHaveBeenCalled();
    expect(rejected).toHaveBeenCalledOnce();
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
  ])('rejects a hostile candidate identity %# before invoking PowerShell', (changed) => {
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
