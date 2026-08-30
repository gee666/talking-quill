import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const rust = resolve('helper/src/windows_installer.rs');
const installer = resolve('build/installer.nsh');

describe('compiled Windows install transaction contract', () => {
  it('routes every production lifecycle operation through the compiled helper', async () => {
    const [source, nsis] = await Promise.all([readFile(rust, 'utf8'), readFile(installer, 'utf8')]);
    expect(nsis).toContain('talking-quill-installer-lifecycle.exe');
    expect(nsis).toContain('--windows-installer-lifecycle-v1=${mode}');
    expect(source).toContain('wait_for_planned_runtime_exit');
    expect(source).toContain('TalkingQuillKeyboardAuthority');
    expect(source).toContain('legacy_task_file');
    expect(source).toContain('legacy_authority');
  });

  it('uses non-vacuous write-ahead ordering checks and production-adapter crash tests', async () => {
    const source = (await readFile(rust, 'utf8')).replace(/\s+/gu, ' ');
    const staging = source.indexOf(
      'write_transaction(&paths.transaction, "staging", true, false, repair)?;',
    );
    const predecessorRename = source.indexOf('fs::rename(&paths.install, &paths.backup)');
    expect(staging).toBeGreaterThanOrEqual(0);
    expect(predecessorRename).toBeGreaterThan(staging);
    const restoring = source.indexOf(
      'write_transaction(&paths.transaction, "restoring", true, false, current.repair)?;',
    );
    const partialRemoval = source.indexOf('remove_plain_tree(&paths.install)?;', restoring);
    expect(restoring).toBeGreaterThanOrEqual(0);
    expect(partialRemoval).toBeGreaterThan(restoring);
    const prepareCall = source.indexOf('prepare_legacy_authority_retirement(paths, machine');
    const durableCommit = source.indexOf('"committed",', prepareCall);
    const finishCall = source.indexOf(
      'finish_legacy_authority_retirement(paths, machine, failure)?;',
      durableCommit,
    );
    const authoritySnapshot = source.indexOf('"authority-preparing"', durableCommit);
    const taskDisable = source.indexOf('machine.set_task_enabled(false)?;', authoritySnapshot);
    const serviceDelete = source.indexOf('machine.delete_service()?;', taskDisable);
    expect(prepareCall).toBeGreaterThanOrEqual(0);
    expect(durableCommit).toBeGreaterThan(prepareCall);
    expect(finishCall).toBeGreaterThan(durableCommit);
    expect(authoritySnapshot).toBeGreaterThanOrEqual(0);
    expect(taskDisable).toBeGreaterThan(authoritySnapshot);
    expect(serviceDelete).toBeGreaterThan(taskDisable);
    const rollback = source.indexOf('fn recover_authority_rollback(');
    const rollbackNeutral = source.indexOf('wait_for_planned_runtime_exit(', rollback);
    const restoreFiles = source.indexOf('restore_predecessor_files(paths, current)?;', rollback);
    const restoreActivity = source.indexOf(
      'restore_legacy_activity(machine, snapshot, failure)?;',
      rollback,
    );
    expect(rollback).toBeGreaterThanOrEqual(0);
    expect(rollbackNeutral).toBeGreaterThan(rollback);
    expect(restoreFiles).toBeGreaterThan(rollbackNeutral);
    expect(restoreActivity).toBeGreaterThan(restoreFiles);
    const activityFunction = source.slice(
      source.indexOf('fn restore_legacy_activity('),
      source.indexOf('fn retire_service_registration('),
    );
    expect(activityFunction).not.toContain('wait_for_planned_runtime_exit');
    for (const test of [
      'fresh_staging_prepared_and_restoring_crashes_are_retryable',
      'runtime_enumeration_waits_for_exact_installed_path_and_propagates_failure',
      'update_crash_points_restore_the_predecessor',
      'commit_quiesces_before_the_durable_decision_and_deletes_after_it',
      'every_precommit_retirement_crash_restores_authority_and_predecessor',
      'running_legacy_authority_crash_recovery_restores_files_before_activity',
      'rollback_recreates_missing_snapshots_and_retires_unexpected_registrations',
      'every_postcommit_retirement_crash_resumes_forward_without_mixed_authority',
    ]) {
      expect(source).toContain(`fn ${test}()`);
    }
    expect(source).not.toContain('production: bool');
  });
});
