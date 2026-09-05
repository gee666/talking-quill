import { readRustModule } from '../helpers/rust-source';
import { readApplicationSource } from '../helpers/application-source';
import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import {
  deriveReleaseBuildDigest,
  encodeReleasePolicy,
} from '../../scripts/macos-owner-policy.mjs';
import {
  validatePhysicalPackageEntries,
  validateResourceEntries,
} from '../../scripts/package-policy.mjs';

const digest = (byte: string) => byte.repeat(64);

async function readMacosCoordinator(): Promise<string> {
  const modules = [
    'macos-owner-update-coordinator',
    'macos-finalizer-supervision',
    'macos-finalizer-status',
    'macos-update-validation',
  ];
  return (
    await Promise.all(modules.map((name) => readFile(`app/src/main/info/${name}.ts`, 'utf8')))
  ).join('\n');
}

describe('macOS R8-M installed owner', () => {
  it('encodes an exact 328-byte enabled macOS policy with one-hop predecessor', () => {
    const policy = encodeReleasePolicy({
      arch: 'arm64',
      buildDigest: digest('1'),
      gatewaySha256: digest('2'),
      ownerSha256: digest('3'),
      gatewayRequirement: 'gateway requirement',
      ownerRequirement: 'owner requirement',
      predecessor: {
        releaseBuildDigest: digest('4'),
        gatewaySha256: digest('5'),
        ownerSha256: digest('6'),
      },
    });
    expect(policy).toHaveLength(328);
    expect(policy.subarray(0, 8).toString('ascii')).toBe('TQKOPOL1');
    expect(policy[10]).toBe(2);
    expect(policy[11]).toBe(2);
    expect(policy[12]).toBe(2);
    expect(policy[13]).toBe(1);
    expect(policy[320]).toBe(2);
    expect(policy[321]).toBe(2);
    expect(policy.subarray(322).equals(Buffer.alloc(6))).toBe(true);
  });

  it('deterministically binds every immutable role and release identity field', () => {
    const identity = {
      packageVersion: '0.0.5',
      arch: 'arm64' as const,
      signingMode: 'self-signed' as const,
      cmsCertificateSha256: digest('1'),
      codeCertificateSha256: digest('2'),
      codeCertificateSha1: '3'.repeat(40),
      gatewaySha256: digest('4'),
      gatewayIdentifier: 'com.talkingquill.app.helper',
      gatewayCdhash: '5'.repeat(40),
      gatewayRequirement: 'gateway requirement',
      ownerSha256: digest('6'),
      ownerIdentifier: 'com.talkingquill.app.keyboard-owner',
      ownerCdhash: '7'.repeat(40),
      ownerRequirement: 'owner requirement',
      bridgeSha256: digest('8'),
      bridgeIdentifier: 'com.talkingquill.app.service-management',
      bridgeCdhash: '9'.repeat(40),
      bridgeRequirement: 'bridge requirement',
      predecessor: {
        releaseBuildDigest: digest('a'),
        gatewaySha256: digest('b'),
        ownerSha256: digest('c'),
      },
    };
    const expected = deriveReleaseBuildDigest(identity);
    expect(expected).toBe('4cb966954db34b59bc531a71ee8caf18ce63c34ebac3b400ae8f5086cc5ba7aa');
    expect(deriveReleaseBuildDigest({ ...identity })).toBe(expected);
    for (const [field, replacement] of [
      ['gatewaySha256', digest('d')],
      ['ownerSha256', digest('e')],
      ['bridgeSha256', digest('f')],
      ['gatewayIdentifier', 'changed.gateway'],
      ['gatewayCdhash', 'a'.repeat(40)],
      ['gatewayRequirement', 'changed gateway requirement'],
      ['ownerIdentifier', 'changed.owner'],
      ['ownerCdhash', 'b'.repeat(40)],
      ['ownerRequirement', 'changed owner requirement'],
      ['bridgeIdentifier', 'changed.bridge'],
      ['bridgeCdhash', 'c'.repeat(40)],
      ['bridgeRequirement', 'changed bridge requirement'],
      ['cmsCertificateSha256', digest('0')],
      ['codeCertificateSha256', digest('a')],
      ['codeCertificateSha1', 'd'.repeat(40)],
      ['signingMode', 'adhoc'],
      ['arch', 'x64'],
      ['packageVersion', '0.0.6'],
    ] as const) {
      expect(deriveReleaseBuildDigest({ ...identity, [field]: replacement })).not.toBe(expected);
    }
    expect(
      deriveReleaseBuildDigest({
        ...identity,
        predecessor: { ...identity.predecessor, ownerSha256: digest('d') },
      }),
    ).not.toBe(expected);
    expect(() => deriveReleaseBuildDigest({ ...identity, gatewaySha256: 'not-a-digest' })).toThrow(
      'gateway SHA-256',
    );
  });

  it('opens nested bundle paths only in explicit owner package mode', () => {
    const nested = [
      'Talking Quill.app/Contents/Library',
      'Talking Quill.app/Contents/Library/LoginItems',
      'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app',
      'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/Info.plist',
    ];
    expect(() => validatePhysicalPackageEntries(nested, 'mac')).toThrow('Unexpected physical');
    expect(() => validatePhysicalPackageEntries(nested, 'mac', { macosOwner: true })).not.toThrow();
    expect(() => validateResourceEntries(['keyboard-owner-r5m.json'], 'mac')).toThrow();
  });

  it('freezes the LoginItem identity, macOS floor, local signing, and no notarization', async () => {
    const [info, config, signer, gateway, owner] = await Promise.all([
      readFile('build/macos-keyboard-owner/Info.plist', 'utf8'),
      readFile('build/electron-builder.macos-owner.yml', 'utf8'),
      readFile('build/mac-owner-sign.cjs', 'utf8'),
      readFile('helper/src/owner/macos/service_management.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/macos/service_management.rs', 'utf8'),
    ]);
    expect(info).toContain('com.talkingquill.app.keyboard-owner');
    expect(info).toContain('<key>LSUIElement</key><true/>');
    expect(info).toContain('<key>LSMinimumSystemVersion</key><string>13.0</string>');
    expect(config).toContain('notarize: false');
    expect(signer).toContain('TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE');
    expect(signer).toContain("'self-signed'");
    expect(signer).toContain('A policy-bound macOS role changed');
    expect(signer.indexOf('await signWithLeastPrivilege')).toBeLessThan(
      signer.indexOf('createInstalledPolicy'),
    );
    expect(signer.indexOf('signRole(gateway')).toBeLessThan(
      signer.indexOf('createInstalledPolicy'),
    );
    expect(signer).not.toContain("join(ownerResources, 'keyboard-owner-r5m.json')");
    const policyScript = await readFile('scripts/macos-owner-policy.mjs', 'utf8');
    expect(policyScript).toContain('observedCertificate');
    expect(policyScript).toContain('compareObservedPin');
    expect(policyScript).toContain('TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256');
    expect(policyScript).toContain('--extract-certificates');
    expect(policyScript).toContain('-certsout');
    expect(policyScript).toContain('-nointern');
    expect(policyScript).toContain('SignerInfo SID');
    expect(policyScript).toContain('Signing identity must resolve uniquely');
    expect(gateway).toContain('Command::new(bridge)');
    expect(owner).toContain('Command::new(&expected.canonical_executable_path)');
    expect(gateway).not.toContain('/dev/fd/100');
    expect(owner).not.toContain('/dev/fd/100');
    expect(gateway).toContain('before.dev() != after.dev()');
    expect(owner).toContain('before.dev() != after.dev()');
    expect(gateway).toContain('validate_spawned_sec_code_identity');
    expect(owner).toContain('validate_spawned_process_against');
    const [gatewayIdentity, ownerIdentity] = await Promise.all([
      readFile('helper/src/owner/macos/identity.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/macos/native_identity.rs', 'utf8'),
    ]);
    expect(gatewayIdentity).toContain('SecCodeCopyGuestWithAttributes');
    expect(gatewayIdentity).toContain('kSecGuestAttributePid');
    expect(gatewayIdentity).toContain('SecCodeCopySigningInformation');
    // Hash-read errors and either CDHash mismatch must take the cleanup/rejection path.
    expect(gatewayIdentity).toMatch(
      /if !matches!\(hashes, Ok\(\(dynamic_cdhash, static_cdhash\)\)\s*if dynamic_cdhash == expected_code_directory_hash && static_cdhash == dynamic_cdhash\)\s*\{\s*release\(dynamic\.cast\(\)\);\s*release\(static_code\.cast\(\)\);\s*release\(requirement_ref\.cast\(\)\);\s*release\(requirement_text\.cast\(\)\);\s*return Err\(IdentityError\);\s*\}/u,
    );
    expect(ownerIdentity).toContain('SecCodeCopyGuestWithAttributes');
    expect(ownerIdentity).toContain('kSecGuestAttributePid');
    expect(gateway).toContain('verify_response');
    expect(owner).toContain('verify_response');
    expect(gateway).not.toContain('loginItemServiceWithIdentifier:');
    expect(owner).not.toContain('unregisterAndReturnError:');
  });

  it('runs Electron ACL denial as an in-process SecItem operation', async () => {
    const [entrypoint, packHook, addon] = await Promise.all([
      readFile('app/src/main/bootstrap.ts', 'utf8'),
      readFile('app/after-pack-macos-owner.cjs', 'utf8'),
      readFile('build/macos-keychain-denial-addon.c', 'utf8'),
    ]);
    expect(entrypoint).toContain('createRequire(import.meta.url)');
    expect(entrypoint).toContain("join(process.resourcesPath, 'macos-keychain-denial.node')");
    expect(entrypoint).toContain('new Set([-25_308, -25_293])');
    expect(entrypoint).not.toMatch(/execFileSync\(bridge, \['acl-denial'\]/u);
    expect(packHook).toContain("'-arch'");
    expect(packHook).toContain("packageArch === 'x64' ? 'x86_64'");
    expect(packHook).toContain("'-bundle'");
    expect(addon).toContain('SecItemCopyMatching');
    expect(addon).toContain('kSecUseAuthenticationUIFail');
  });

  it('keeps credentials out of argv/environment and requires no-UI Keychain access', async () => {
    const [connector, keychain, provisioning] = await Promise.all([
      readFile('helper/src/owner/macos/connector.rs', 'utf8'),
      readFile('helper/src/owner/macos/keychain.rs', 'utf8'),
      readFile('helper/src/owner/macos/provisioning.rs', 'utf8'),
    ]);
    expect(keychain).toContain('kSecUseAuthenticationUIFail');
    expect(provisioning).toContain('SecTrustedApplicationCreateFromPath');
    expect(provisioning).toContain('SecKeychainItemSetAccess');
    expect(provisioning).toContain('validate_unique_item');
    expect(keychain).toContain('kSecMatchLimitAll');
    expect(connector).not.toMatch(/std::env::var.*secret|--secret|SECRET=/u);
  });

  it('wires predecessor-bound update, rollback, held-key postponement, and uninstall cleanup', async () => {
    const application = readApplicationSource();
    const [electron, native, helperEntrypoint] = await Promise.all([
      readMacosCoordinator(),
      readFile('helper/src/owner/macos/maintenance.rs', 'utf8'),
      readFile('helper/src/main.rs', 'utf8'),
    ]);
    expect(electron).toContain('prepareOwnerMaintenance');
    expect(electron).toContain('waitForAuthenticatedStatus');
    expect(electron).toContain('boundedTerminate(child');
    expect(electron).toContain("stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe', 'pipe']");
    expect(electron).toContain('cancellation.end()');
    expect(electron).toContain('awaitAuthenticatedRollbackOrTerminate');
    expect(electron).toContain('NATIVE_RECOVERY_SUPERVISION_MS = 150_000');
    expect(electron).toContain('boundedTerminate');
    expect(electron).toContain("child.kill('SIGKILL')");
    expect(electron.indexOf('cancellationPipe.end()')).toBeLessThan(
      electron.indexOf('await awaitAuthenticatedRollbackOrTerminate'),
    );
    expect(electron).toContain('prepareRollback');
    expect(electron).toContain('prepareUninstall');
    expect(electron).toContain('Release all Talking Quill shortcut keys');
    expect(native).toContain('policy.predecessor');
    expect(native).toMatch(/MacosLoginItemService\s*\.unregister/u);
    expect(native).toContain('delete_after_unregistration');
    expect(native).toContain('prepare_uninstall_staging_for_last_backup');
    expect(native).toContain('finish_committed_uninstall');
    expect(native).toContain('uninstall_cleanup_pending');
    expect(native).toContain('UninstallCleanupJournal');
    expect(native).toContain('source_release_policy_signature');
    expect(native).toContain('resume_committed_uninstall_cleanup');
    expect(native).toContain(
      'Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(())',
    );
    expect(native).toContain('ResumeCredentialAction::EstablishDurableCommit');
    expect(native).toContain('Tombstone authority begins only after absence is proven');
    expect(native).toContain('crash_before_keychain_deletion_never_authorizes_tombstone_removal');
    expect(native).toContain('cleanup-v1/{}.app');
    expect(native).toContain('super::cms::verify');
    expect(native).toContain('read_regular(&path, 128 * 1024)');
    expect(native.indexOf('persist_cleanup_pending(journal_path, journal)')).toBeLessThan(
      native.indexOf('remove_exact_cleanup_tombstone(&tombstone)'),
    );
    expect(native).toContain('Err(_error) if uninstall_committed');
    expect(native).toContain('delete_durable(request)');
    expect(native.indexOf('prepare_uninstall_staging_for_last_backup(&request)')).toBeLessThan(
      native.indexOf('uninstall_committed = true'),
    );
    expect(native.indexOf('uninstall_committed = true')).toBeLessThan(
      native.indexOf('finish_committed_uninstall(&request, &retained_backup, cleanup_deadline)'),
    );
    expect(native.match(/Duration::from_secs\(120\)/gu)).toHaveLength(1);
    expect(native).toContain('wait_for_path_until(');
    expect(native).toContain('reporter.cancellation_state()');
    expect(native).toContain('begin_credential_commit()?');
    expect(native).toContain('compare_exchange(CANCELLABLE, COMMITTING');
    expect(native).toContain('monitor.cancel()');
    expect(native).toContain('No caller may create another deadline');
    const committedError = native.slice(
      native.indexOf('Err(_error) if uninstall_committed'),
      native.indexOf('Err(error) =>'),
    );
    expect(committedError).not.toContain('finish_committed_uninstall');
    expect(committedError).toContain('reporter.cleanup_pending()');
    const committedCleanup = native.slice(
      native.indexOf('fn finish_committed_uninstall'),
      native.indexOf('enum TombstoneCleanup'),
    );
    expect(committedCleanup).not.toContain('write_marker_if_missing');
    expect(committedCleanup).toContain('delete_durable(request)');
    const durableDelete = native.slice(
      native.indexOf('fn delete_durable'),
      native.indexOf('fn write_durable'),
    );
    expect(durableDelete).toContain('ErrorKind::NotFound');
    expect(native).toContain('FinalizerReporter');
    expect(native).toContain('authenticate_request(&request, &config, owner_handoff)');
    expect(native).toContain('copy_predecessor(');
    expect(native).toContain('&trusted_outer');
    expect(native).toContain('recover_after_unregistration_under_lock');
    expect(native).toContain('recover_after_unregistration_unlocked');
    expect(native).toContain('commit_rollback_before_authority');
    expect(native).toContain('rollback_journal_write_or_sync_failure_never_restores_authority');
    const rollbackRecovery = native.slice(
      native.indexOf('fn recover_after_unregistration_under_lock'),
      native.indexOf('enum RecoveryAction'),
    );
    expect(rollbackRecovery.indexOf('write_durable(request, "rolled_back")')).toBeLessThan(
      rollbackRecovery.indexOf('write_maintenance_latch'),
    );
    expect(rollbackRecovery.indexOf('write_maintenance_latch')).toBeLessThan(
      rollbackRecovery.indexOf('super::provisioning::provision_if_missing(config)'),
    );
    expect(native).toContain('RECOVERY_AUTHORITY_DEADLINE');
    expect(native).toContain('Keep the synced RolledBack journal/latch');
    expect(native).toContain('reporter.cancellation_state()');
    expect(native).toContain('tq-finalizer-cancel');
    expect(native).toContain('terminate_child_bounded');
    expect(native).toContain('cleanup_pending()');
    expect(native).toContain('reporter.uninstall_ready()?');
    expect(native).toContain('reporter.complete()?');
    expect(electron).toContain("['uninstall_ready', 'error']");
    expect(electron).toContain("['complete', 'cleanup_pending', 'error']");
    expect(electron).toContain('UNINSTALL_TERMINAL_SUPERVISION_MS = 150_000');
    expect(application).toContain('prepareUninstall(() =>');
    expect(native).toContain('MacosMaintenancePhase::InstallationComplete');
    expect(native.indexOf('super::cms::verify')).toBeLessThan(
      native.indexOf('.signing\n        .requirement'),
    );
    expect(native).toContain('policy != owner_policy');
    expect(native.indexOf('drop(lock);')).toBeLessThan(
      native.indexOf('MacosLoginItemService.ensure_registered(&config)'),
    );
    expect(native).toContain('write_durable(&request, "complete")');
    expect(application).toContain('--uninstall-local-owner');
    expect(application).toContain('--macos-owner-resume-cleanup');
    expect(helperEntrypoint).toContain('--macos-owner-resume-cleanup');
    expect(helperEntrypoint).toContain('resume_committed_uninstall_cleanup');
    expect(application.indexOf('--macos-owner-resume-cleanup')).toBeLessThan(
      application.indexOf('new DataLifecycleService'),
    );
  });

  it('carries canonical digest bytes and owner handoff without argv exposure', async () => {
    const [coordinator, gateway, owner, schema] = await Promise.all([
      readMacosCoordinator(),
      readFile('helper/src/owner/platform_client.rs', 'utf8'),
      readRustModule('helper/keyboard-owner/src/protocol_server.rs'),
      readFile('app/src/shared/helper/protocol.ts', 'utf8'),
    ]);
    expect(gateway).toContain('fn maintenance_digest');
    expect(gateway).toContain('Bytes32::new(bytes)');
    expect(gateway).not.toContain('fn digest_text');
    expect(owner).toContain('owner_handoff: Bytes32::new');
    expect(schema).toContain('ownerHandoff');
    expect(coordinator).toContain("stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe', 'pipe']");
    expect(coordinator).toContain('handoffPipe.end');
    expect(coordinator).toContain("Buffer.from(ownerHandoff, 'hex')");
    expect(coordinator).not.toMatch(/--macos-owner-finalize[^\n]*ownerHandoff/u);
  });

  it('binds the outer ZIP identity and validates the complete candidate bundle natively', async () => {
    const application = readApplicationSource();
    const [coordinator, native, macho] = await Promise.all([
      readMacosCoordinator(),
      readFile('helper/src/owner/macos/maintenance.rs', 'utf8'),
      readFile('helper/src/macho.rs', 'utf8'),
    ]);
    expect(application).toContain('release-identity-mac-${process.arch}.json');
    expect(application).toContain('parseUnsignedUpdateIdentity');
    expect(coordinator).toContain('Exact macOS updater identity is required');
    expect(coordinator).toContain('identity.packageSha256 !== input.archiveSha256');
    expect(coordinator).toContain('keyboard-owner-release-v1.json');
    expect(native).toContain('verify_outer_bundle');
    expect(native).toContain('"--verify",');
    expect(native).toContain('&format!("-R={}", trusted.designated_requirement)');
    expect(native).toContain('trusted_predecessor_outer_identity');
    expect(native).toContain('matches_trusted(&observed, trusted)');
    expect(native).toContain('verify_bundle_architectures(app, architecture)');
    expect(native).toContain('crate::macho::exact_architecture');
    expect(macho).toContain('fat32_and_fat64_require_one_exact_slice');
    expect(macho).toContain('CPU_TYPE_X86_64, 18');
    expect(native).toContain('&request.installed_app');
    expect(native).toContain('request.architecture.as_deref()');
  });

  it('authenticates gateway policy and both native identities before mutation', async () => {
    const [config, connector] = await Promise.all([
      readFile('helper/src/owner/macos/config.rs', 'utf8'),
      readFile('helper/src/owner/macos/connector.rs', 'utf8'),
    ]);
    expect(config.match(/super::cms::verify/g)).toHaveLength(3);
    expect(config).toContain('gateway_policy != owner_policy');
    expect(config).toContain('validate_native_sec_code_identity');
    expect(config).toContain('stable_hash(&expected_owner)');
    expect(config).toContain('stable_hash(&expected_bridge)');
    expect(config).toContain('bridge_release_policy');
    expect(connector.indexOf('InstalledConfig::load')).toBeLessThan(
      connector.indexOf('provision_if_missing'),
    );
  });

  it('gates coordinator availability independently from updater metadata', () => {
    const application = readApplicationSource();
    expect(application).toContain('keyboard-owner-installed-v1');
    expect(application).toContain('keyboard-owner-r5m.json');
    expect(application).toContain('installedMacosOwnerAvailable');
    expect(application).toContain(
      "process.platform !== 'darwin' || macosUpdateCoordinator !== null",
    );
    expect(application).toContain('localOwnerMaintenanceRequested');
    expect(application).toContain('installed macOS owner maintenance coordinator is unavailable');
    expect(application).toContain('constants.O_NOFOLLOW');
    expect(application).toContain('--macos-owner-validate-install');
    expect(application).not.toContain(
      "existsSync(join(process.resourcesPath, 'keyboard-owner-r5m.json'))",
    );
  });

  it('keeps Drag-to-Trash cleanup poisoned and capture closed until durable cleanup', async () => {
    const [entrypoint, removal, buildGate, inspector] = await Promise.all([
      readFile('helper/keyboard-owner/src/main.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/macos/removal.rs', 'utf8'),
      readFile('helper/keyboard-owner/build.rs', 'utf8'),
      readFile('scripts/inspect-package.mjs', 'utf8'),
    ]);
    expect(entrypoint).toContain('removal_poisoned');
    expect(entrypoint.indexOf('removal_poisoned')).toBeLessThan(
      entrypoint.indexOf('MacosEndpointConfig::load_for_current_install'),
    );
    expect(entrypoint).toContain('finish_removed_install');
    expect(entrypoint).toContain('finish_poisoned_without_bundle');
    expect(removal).toContain('fixed_items_absent_without_ui');
    expect(removal).toContain('O_NOFOLLOW');
    expect(removal).toContain('removal-required-v1');
    expect(removal).toContain('cleanup_once(runtime, bridge)?');
    expect(removal).toContain('remove_validated_runtime_component');
    expect(removal).toContain('ErrorKind::NotFound');
    expect(removal).toContain('idempotent cleanup after NotFound');
    expect(entrypoint).toContain('loop {');
    expect(entrypoint).toContain('Capture can never reopen in this process');
    expect(entrypoint).toContain('removal_bridge = replacement');
    expect(removal).toContain('fs::remove_file(&poison)');
    expect(removal).toContain('LIFECYCLE_FIXTURE_ATTEMPT.fetch_add');
    expect(removal).toContain('permissioned-ci-v1');
    expect(buildGate).toContain('drag-to-trash-fixture-');
    expect(buildGate).toContain('CARGO_FEATURE_MACOS_NATIVE_LIFECYCLE_FIXTURE');
    expect(inspector).toContain('Packaged owner contains lifecycle test hook');
    const gatewayMaintenance = await readFile('helper/src/owner/macos/maintenance.rs', 'utf8');
    expect(gatewayMaintenance).toContain('resume_durable_removal_if_needed');
    expect(gatewayMaintenance).toContain('fixed_items_absent');
  });

  it('documents restart-resumable cleanup and the honest absent-code limitation', async () => {
    const documentation = await readFile('docs/macos-local-owner-install.md', 'utf8');
    expect(documentation).toContain('--macos-owner-resume-cleanup');
    expect(documentation).toContain('needs no deleted Keychain item');
    expect(documentation).toContain(
      'If the application is truly absent, no application code can execute',
    );
    expect(documentation).toContain('next reinstall or launch always resumes');
  });

  it('runs Bash-3.2-compatible exact packaged and ignored native gates', async () => {
    const workflow = await readFile('.github/workflows/build-mac-local-owner.yml', 'utf8');
    expect(workflow).not.toContain('mapfile');
    expect(workflow).toContain('--ignored --exact --nocapture');
    expect(workflow).toContain('exact_packaged_artifact_identity_and_bridge_policy_validate');
    expect(workflow).toContain(
      'native_keychain_zero_one_duplicate_malformed_and_ui_fail_semantics',
    );
    expect(workflow).toContain('--bin macos-keychain-fixture');
    expect(workflow).not.toContain('missing native Keychain corpus fixture');
    expect(workflow).toContain('talking-quill-permissioned');
    expect(workflow).toContain('"$bridge" acl-denial');
    expect(workflow).toContain('uninstall_cleanup_pending');
    expect(workflow).not.toContain('security find-generic-password');
    expect(workflow).toContain('serve-authenticated');
    expect(workflow).toContain("f[3] !== 'handshake-present'");
    expect(workflow).toContain("f[3] !== 'fixed-absent'");
    expect(workflow).toContain("f[4] !== '-25300'");
    expect(workflow).toContain('EXPECTED_PID="$keychain_fixture_pid"');
    expect(workflow).toContain('EXPECTED_AUDIT="$keychain_fixture_audit"');
    expect(workflow).toContain('talking-quill/macos-keychain-fixture/v1');
    expect(workflow).toContain("f[2] !== '4'");
    expect(workflow).toContain('drag_owner_pid injected');
    expect(workflow).toContain('direct_bridge_status');
    expect(workflow).toContain('cannot manufacture maintenance');
    const bridgeBinary = await readFile(
      'helper/src/bin/talking-quill-macos-service-bridge.rs',
      'utf8',
    );
    const bridgeAuthority = await readFile('helper/src/macos_service_bridge.rs', 'utf8');
    expect(bridgeBinary).toContain('validate_parent_process');
    expect(bridgeAuthority).toContain('libc::getppid()');
    expect(bridgeAuthority).toContain('validate_bridge_parent');
    expect(bridgeAuthority).toContain('parent supplies no path, hash, CDHash, or requirement');
    expect(workflow).toContain('lipo -archs');
    expect(workflow).not.toContain('rollback_build');
    expect(workflow).not.toContain('tmp/native-rollback.zip');
    expect(workflow).not.toContain(
      "rm -rf '/Applications/Talking Quill.app'\n          ditto --noqtn 'tmp/native-packaged-artifact",
    );
    expect(workflow).toContain('Exact rollback to the');
    expect(workflow).toContain('R11 installed lifecycle');
    expect(workflow).toContain('TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE');
    expect(workflow).not.toContain('talking-quill-native-lifecycle');
    expect(workflow).toContain('Hosted runners never claim');
  });

  it('gates every installed owner startup on exact Keychain completion reconciliation', async () => {
    const [entrypoint, config, adapter, record] = await Promise.all([
      readFile('helper/keyboard-owner/src/main.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/macos/config.rs', 'utf8'),
      readFile('helper/keyboard-owner/src/platform_adapter.rs', 'utf8'),
      readFile('helper/owner-protocol/src/macos_maintenance.rs', 'utf8'),
    ]);
    expect(entrypoint).toContain('reconcile_startup_maintenance');
    expect(config).toContain('MacosMaintenancePhase::InProgress');
    expect(config).toContain('clear_maintenance_record_without_ui');
    expect(adapter).toContain('MacosMaintenanceRecord::in_progress');
    expect(adapter).toContain('_request.owner_handoff()');
    const ownerServer = await readRustModule('helper/keyboard-owner/src/protocol_server.rs');
    expect(ownerServer).toContain('Bytes32::random()');
    expect(record).toContain('owner_handoff');
    expect(record).toContain('MACOS_MAINTENANCE_RECORD_BYTES');
  });
});
