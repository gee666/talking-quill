import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const source = readFileSync('installer/windows-setup/src/windows.rs', 'utf8');
const cargo = readFileSync('installer/windows-setup/Cargo.toml', 'utf8');
const helperCargo = readFileSync('helper/Cargo.toml', 'utf8');
const helperSource = readFileSync('helper/src/windows_update.rs', 'utf8');
const testGuard = readFileSync('scripts/run-machine-lock-isolated-tests.mjs', 'utf8');
const packageJson = readFileSync('package.json', 'utf8');
const productionBuild = readFileSync('scripts/build-windows-setup.mjs', 'utf8');
const helperBuild = readFileSync('scripts/build-helper.mjs', 'utf8');

function index(text: string): number {
  const value = source.indexOf(text);
  expect(value, `missing ${text}`).toBeGreaterThan(-1);
  return value;
}

describe('Windows schema-2 stale coordination cleanup', () => {
  it('isolates machine-lock tests under randomized project tmp and HKCU namespaces', () => {
    expect(cargo).toContain('machine-lock-test-namespace = []');
    expect(helperCargo).toContain('machine-lock-test-namespace = []');
    for (const implementation of [source, helperSource]) {
      expect(implementation).toContain('TQ_MACHINE_LOCK_TEST_NAMESPACE_ID');
      expect(implementation).toContain('HKEY_CURRENT_USER');
      expect(implementation).toContain('tmp/machine-lock-tests/');
      expect(implementation).toContain('Local\\TalkingQuill.Tests.');
      expect(implementation).toContain(
        '#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]',
      );
    }
    expect(testGuard).toContain('Global\\\\TalkingQuill.MachineLockTests.V1');
    expect(testGuard).toContain("assertNoTestNamespaceLeftovers('before')");
    expect(testGuard).toContain("assertNoTestNamespaceLeftovers('after')");
    expect(testGuard).toContain('productionResidueSnapshot()');
    expect(testGuard).toContain('removeKnownLeakedTestNamespace');
    expect(testGuard).toContain('removeEmptyTestRegistryRoot');
    expect(testGuard).toContain('HKEY_LOCAL_MACHINE\\Software\\Talking Quill Tests');
    for (const implementation of [source, helperSource]) {
      expect(implementation).toContain(
        '.expect("machine-lock tests require the wrapper namespace environment")',
      );
      expect(implementation).not.toContain('test namespace randomness');
    }
    expect(packageJson).toContain('run-machine-lock-isolated-tests.mjs');
    expect(productionBuild).toContain("'TQ_MACHINE_LOCK_TEST_NAMESPACE_ID'");
    expect(productionBuild).toContain("'Talking Quill Tests'");
    expect(helperBuild).toContain("'TQ_MACHINE_LOCK_TEST_NAMESPACE_ID'");
    expect(helperBuild).toContain('Windows helper contains machine-lock test marker');
    expect(helperSource).toContain(
      'production_machine_lock_constructor_admits_and_retires_exact_published_tree',
    );
  });

  it('keeps medium cleanup on authenticated UAC and production builds closed', () => {
    expect(cargo).toContain('stale-schema2-cleanup = []');
    expect(source).toContain('#[cfg(feature = "stale-schema2-cleanup")]\n    CleanStaleSchema2');
    expect(source).toContain(
      '#[cfg(not(feature = "stale-schema2-cleanup"))]\n    let cleanup_requested = false;',
    );
    expect(source).toContain(
      '#[cfg(not(feature = "stale-schema2-cleanup"))]\n    let direct_cleanup_requested = false;',
    );
    expect(source).toContain('WorkerChannel::connect_and_authenticate(&current, None)?');
    expect(index('requested_action == Some(Action::CleanStaleSchema2)')).toBeGreaterThan(
      index('WorkerChannel::connect_and_authenticate(&current, None)?'),
    );
    expect(source).toContain('let result = elevate(&current, true, &channel, None);');
    expect(source).toContain('if !token_is_elevated()?');
  });

  it('admits only the exact direct command from a high elevated feature build', () => {
    expect(source).toContain(
      'elevated && arguments.len() == 1 && arguments[0] == "/TQ-CLEAN-STALE-SCHEMA2";',
    );
    expect(index('if direct_cleanup_requested {')).toBeLessThan(index('if !elevated {'));
    expect(source).toContain('return run_direct_elevated_stale_schema2_cleanup()');
    expect(source).toContain('.map_err(|error| fail(EXIT_REJECTED, error.message));');
    expect(source).toContain('const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;');
    expect(source).toContain('integrity_rid >= SECURITY_MANDATORY_HIGH_RID');
    expect(source).toContain(
      'peer_claims(std::process::id()).map(|claims| (elevated, claims.integrity_rid))',
    );
    expect(source).toContain('"Direct stale cleanup requires a high elevated token."');
    expect(source).toContain('direct_stale_cleanup_rejects_wrong_arguments_and_token_modes');
  });

  it('binds direct cleanup to the running package image and compiled source identity', () => {
    expect(source).toContain('canonical(&current)? != canonical(&kernel_image)?');
    expect(source).toContain('.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)');
    expect(source).toContain('let package = package::parse(&mut image, length)');
    expect(source).toContain('option_env!("TALKING_QUILL_RELEASE_COMMIT")');
    expect(source).toContain('option_env!("TALKING_QUILL_RELEASE_TREE")');
    expect(source).toContain('package.manifest.source_commit != source_commit');
    expect(source).toContain('package.manifest.source_tree != source_tree');
    expect(source).toContain('package.manifest.package_mode != "stale-schema2-cleanup"');
    expect(source).toContain('package.manifest.predecessor.is_some()');
    expect(source).toContain('package.manifest.fault_phase.is_some()');
    expect(source).toContain('!staged_path_is_protected(parent, true)?');
    expect(source).toContain('!protected_file_handle_acl_is_exact(&image)?');
    expect(source).toContain('file_identity_text(&path_image)? != identity');
    expect(source).toContain('hash_reader(&mut retained_image)? != expected_hash');
  });

  it('rejects missing, relative, unprotected, or replaceable audit paths', () => {
    expect(source).toContain('TQ_STALE_SCHEMA2_AUDIT_PATH is required.');
    expect(source).toContain('if !path.is_absolute()');
    expect(source).toContain('share_mode(FILE_SHARE_READ)');
    expect(source).toContain(
      'custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)',
    );
    expect(source).toContain('if !protected_file_handle_acl_is_exact(&file)?');
    expect(source).toContain('"Cleanup audit is not administrator protected."');
    expect(source).toContain(
      'direct_stale_cleanup_rejects_missing_relative_and_unprotected_audits',
    );
  });

  it('pins the exact unpublished synthetic record bytes and hash', () => {
    const literal = /const SYNTHETIC_SCHEMA2_BYTES: &\[u8\] = br#"(.+?)"#;/s.exec(source)?.[1];
    expect(literal).toBeDefined();
    expect(JSON.parse(literal ?? '')).toMatchObject({
      schemaVersion: 2,
      generation: '78bd88811b14faf1e11ba59620088aa0',
      request: '--windows-update-bootstrap-v2=dGVzdA==',
      sourceVersion: '0.0.69',
      targetVersion: '0.0.70',
      phase: 'armed',
    });
    expect(
      createHash('sha256')
        .update(literal ?? '')
        .digest('hex'),
    ).toBe('abb2d6183c58b6ec52e28f6befbe43d949d2eeaf1998122921118272da8f3bad');
    expect(source).toContain('bytes != SYNTHETIC_SCHEMA2_BYTES');
    expect(source).toContain('hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256');
  });

  it('fails closed on owners and takes locks in the production order', () => {
    for (const check of [
      'registry_key_present(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?',
      'registry_key_present(HKEY_LOCAL_MACHINE, APP_PATH_KEY)?',
      'let run_absent = no_owned_run_values()?;',
      'no_talking_quill_process_except_authenticated_pair(authenticated_parent)?;',
      'let services_absent = no_owned_service_keys()?;',
      'Tasks/TalkingQuillKeyboardAuthority',
      '.Talking Quill.native-transaction-v2.json',
    ]) {
      expect(source).toContain(check);
    }
    const cleanup = source.slice(
      index('fn reclaim_exact_schema2_orphan_v2('),
      index('fn reclaim_exact_schema2_orphan_with_audit('),
    );
    expect(cleanup.indexOf('let legacy = LegacyMutexPair::acquire()?;')).toBeLessThan(
      cleanup.indexOf(
        'RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?',
      ),
    );
    expect(
      cleanup.indexOf(
        'RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?',
      ),
    ).toBeLessThan(cleanup.indexOf('let admission = active_state_proof('));
    expect(source).toContain('share_mode(0)');
    expect(source).toContain('Duration::from_millis(750)');
  });

  it('admits only the exact retained legacy registry descriptor and hardens it structurally', () => {
    expect(source).toContain(
      'OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION',
    );
    expect(source).toContain('STALE_REGISTRY_LEGACY_SDDL');
    expect(source).toContain('child_sddl == legacy && parent_sddl == legacy');
    expect(source).toContain('LegacyExactParent');
    expect(source).toContain('KEY_READ | KEY_WRITE | WRITE_DAC | WRITE_OWNER');
    expect(source).toContain('REG_OPTION_OPEN_LINK');
    expect(source).toContain(
      'REGISTRY_DESCRIPTOR_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION',
    );
    expect(source).toContain('GetSecurityDescriptorOwner');
    expect(source).toContain('GetSecurityDescriptorGroup');
    expect(source).toContain('GetSecurityDescriptorDacl');
    expect(source).toContain('GetSecurityDescriptorControl');
    expect(source).toContain('GetAce');
    expect(source).toContain('Cannot reopen protected stale registry state.');
    expect(source).toContain('Protected stale registry descriptor did not verify structurally.');
    expect(source).toContain('exact_current_inherited_registry_descriptor_requires_its_parent');
    expect(source).toContain('stale_registry_acl_reorder_and_extra_ace_are_rejected');
    expect(source).toContain('stale_registry_value_and_suffix_inventory_is_exact');
    expect(source).toContain('hardened_registry_descriptor_is_structurally_exact');
  });

  it('checks exact inventories, removes through owned-tree identities, and deletes registry last', () => {
    expect(source).toContain('Retained stale fixture inventory is not exact.');
    expect(source).toContain('"publication-pending-v1"');
    expect(source).toContain('Published machine lock marker changed during stability wait.');
    expect(source).toContain('publication_pending.delete()?');
    expect(source).toContain('pending.delete()?');
    expect(source).toContain('lifecycle.finish_deleted()?');
    const reclaim = source.slice(index('fn reclaim_exact_schema2_orphan_v2('));
    const schemaMutation = reclaim.indexOf('pending.delete()?');
    const schemaRegistryDelete = reclaim.indexOf(
      'stale machine lifecycle publication',
      schemaMutation,
    );
    const schemaRename = reclaim.indexOf(
      'lifecycle.rename(&retained_lifecycle_path)?',
      schemaMutation,
    );
    const schemaFinish = reclaim.indexOf('lifecycle.finish_deleted()?', schemaRegistryDelete);
    expect(schemaMutation).toBeLessThan(schemaRename);
    expect(schemaRename).toBeLessThan(schemaRegistryDelete);
    expect(schemaFinish).toBeGreaterThan(schemaRegistryDelete);
    expect(source).toContain(
      'let zero = active_state_proof(\n        program_files,\n        program_data,\n        system,\n        false,\n        authenticated_parent,\n    )?;',
    );
    expect(source).toContain('audit.record("completed", binding, &zero)');
    expect(source).toContain('reclaim_exact_schema2_orphan_with_audit(true, false, &mut audit)?;');
    expect(source).toContain('reclaim_exact_schema2_orphan(true, true)?;');
    expect(source).toContain(
      'no_talking_quill_process_except_authenticated_pair(authenticated_parent)?;',
    );
  });

  it('limits production reclaim to authenticated fresh installs before paths create state', () => {
    const production = index(
      'package.manifest.package_mode == "fresh" && requested_action == Some(Action::Install)',
    );
    expect(production).toBeGreaterThan(index('TQPKG2 architecture does not match'));
    expect(production).toBeLessThan(index('let mut paths = paths()?;'));
    expect(source).toContain('TQ_STALE_SCHEMA2_AUDIT_PATH is required.');
  });

  it('requires a protected completion audit and full zero proof on every successful branch', () => {
    const reclaimStart = index('fn reclaim_exact_schema2_orphan_v2(');
    const reclaim = source.slice(
      reclaimStart,
      source.indexOf('\nfn reclaim_exact_schema2_orphan_with_audit(', reclaimStart),
    );
    expect(reclaim).not.toContain('StaleCleanupAudit::open()');
    expect(reclaim).toContain('audit: &mut StaleCleanupAudit');
    expect(source.match(/let mut audit = StaleCleanupAudit::open\(\)\?;/g)).toHaveLength(1);
    expect(source).toContain('audit.record("inspected", &binding, &admission)?;');
    expect(source).toContain('audit.record("commit-intent", &binding, &second)?;');
    expect(source).toContain('audit.record(stage, &empty, &empty)');
    const auditImplementation = source.slice(
      index('impl StaleCleanupAudit {'),
      index('fn retained_binding('),
    );
    expect(auditImplementation).not.toContain('FILE_SHARE_WRITE');
    expect(source).toContain('force_stale_cleanup_rejection("post-inspected")?;');
    expect(source).toContain('force_stale_cleanup_rejection("post-commit-intent")?;');
    expect(source).toContain(
      'forced_post_inspected_and_post_commit_rejections_keep_one_audit_chain',
    );
    expect(reclaim).toContain('retained_binding(&[], "no-machine-lock-publication")');
    expect(reclaim.match(/complete_stale_cleanup_zero_state\(/g)).toHaveLength(3);
    expect(reclaim.match(/return Ok\(\(\)\);/g)).toHaveLength(2);
    expect(reclaim).toContain('exact_stale_coordination_inventory(&program_data, &suffix, false)?');
    expect(reclaim).toContain('exact_stale_coordination_inventory(&program_data, &suffix, true)?');
    expect(reclaim.trimEnd().endsWith('Ok(())\n}')).toBe(true);
  });
});
