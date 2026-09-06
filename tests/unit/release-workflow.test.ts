import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const producerWorkflow = readFileSync(
  '.github/workflows/windows-installed-acceptance-producer.yml',
  'utf8',
);
const acceptanceProducer = readFileSync(
  'scripts/windows-installed-acceptance-native-build.mjs',
  'utf8',
);
const publishWorkflow = readFileSync('.github/workflows/publish-local-owner.yml', 'utf8');
const realRebootWorkflow = readFileSync(
  '.github/workflows/windows-real-reboot-acceptance.yml',
  'utf8',
);
const stageScript = readFileSync('scripts/stage-unsigned-release.mjs', 'utf8');
const assembleScript = readFileSync('scripts/assemble-release.mjs', 'utf8');

function section(start: string, end?: string): string {
  const startIndex = workflow.indexOf(`\n  ${start}:`);
  const endIndex = end === undefined ? workflow.length : workflow.indexOf(`\n  ${end}:`);
  expect(startIndex, start).toBeGreaterThan(-1);
  expect(endIndex, end).toBeGreaterThan(startIndex);
  return workflow.slice(startIndex, endIndex);
}

describe('Windows native release workflow', () => {
  it('uses an input-free, secret-free ordinary release path', () => {
    expect(workflow.slice(0, workflow.indexOf('permissions:'))).not.toContain('inputs:');
    for (const source of [workflow, publishWorkflow]) {
      expect(source).not.toContain('secrets.');
      expect(source).not.toContain('environment:');
      expect(source).not.toContain('release-control-preflight.mjs');
      expect(source).not.toContain('windows-promotion-evidence.mjs');
      expect(source).not.toContain('publication-manifest.mjs');
      expect(source).not.toContain('reboot_evidence_');
      expect(source).not.toContain('installed_acceptance_x64_run_id');
      expect(source).not.toContain('v0.0.69');
    }
    const producerName = /^name: (.+)$/mu.exec(workflow)?.[1];
    expect(publishWorkflow).toContain(`workflows: [${String(producerName)}]`);
    expect(publishWorkflow).toContain('types: [completed]');
    expect(publishWorkflow).toContain("github.event.workflow_run.conclusion == 'success'");
    expect(publishWorkflow).toContain('github.event.workflow_run.id');
    expect(publishWorkflow).toContain('gh release edit');
  });

  it('disables automatic package caching in dependency-free Node jobs', () => {
    expect(section('smoke', 'assemble')).toContain('package-manager-cache: false');
    expect(section('assemble')).toContain('package-manager-cache: false');
    expect(publishWorkflow.match(/package-manager-cache: false/gu)).toHaveLength(2);
  });

  it('runs validation and builds only the current fresh trust-root package', () => {
    const validate = section('validate', 'package');
    const packageJob = section('package', 'smoke');
    expect(validate).toContain('pnpm validate:unsigned-release');
    expect(validate).toContain('pnpm security:gate');
    expect(packageJob).toContain('package_script: package:win');
    expect(packageJob).toContain('package_script: package:win:arm64');
    expect(packageJob).toContain('TALKING_QUILL_PACKAGE_MODE: fresh');
    expect(packageJob).toContain("TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1'");
    expect(packageJob).toContain("TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT: '1'");
    expect(packageJob.match(/TALKING_QUILL_PACKAGE_MODE: fresh/gu)).toHaveLength(1);
    expect(packageJob.match(/TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1'/gu)).toHaveLength(1);
    expect(packageJob.match(/TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT: '1'/gu)).toHaveLength(1);
    expect(packageJob).toContain("'^TALKING_QUILL_(?:MACOS_)?PREDECESSOR_'");
    expect(packageJob).toContain('Remove-Item "Env:$($_.Name)"');
    expect(packageJob.indexOf('Remove-Item "Env:$($_.Name)"')).toBeLessThan(
      packageJob.indexOf('pnpm --filter @talking-quill/app ${{ matrix.package_script }}'),
    );
    expect(packageJob).not.toContain('Build explicit predecessor-bound update');
    expect(workflow).not.toContain('predecessor_x64_');
    expect(workflow).not.toContain('predecessor_arm64_');
    expect(workflow).not.toContain('PREDECESSOR_INSTALLER_URL');
    expect(stageScript).toContain('windowsFreshTrustRoot');
    expect(assembleScript).toContain('freshTrustRoot');
    expect(assembleScript).toContain('fresh trust-root provenance identity mismatch');
    expect(packageJob).not.toContain('latest-${{ matrix.arch }}.yml');
    expect(packageJob).not.toContain('release-identity-win-${{ matrix.arch }}.json');
    expect(packageJob).toContain('Stage exact Windows fresh payload');
    expect(packageJob).toContain('stage-unsigned-release.mjs win ${{ matrix.arch }}');
    expect(packageJob).not.toContain('UPDATE_KEY_INPUT');
    expect(packageJob).not.toContain('WINDOWS_UPDATE_SIGNING_KEY');
    expect(packageJob).not.toContain('key-import');
    expect(packageJob).not.toContain('key-delete');
    expect(packageJob).not.toContain('--update-private-key');
  });

  it('runs the real acceptance producer under release-signing and unconditionally retires keys', () => {
    expect(producerWorkflow).toContain('environment: release-signing');
    expect(producerWorkflow).toContain('windows-x64-exact-native-setup-input');
    expect(producerWorkflow).toContain(
      'node scripts/run-windows-installed-acceptance-build-e2e.mjs',
    );
    expect(producerWorkflow).toContain('Remove-Item Env:UPDATE_KEY_INPUT');
    expect(producerWorkflow.indexOf('Remove-Item Env:UPDATE_KEY_INPUT')).toBeLessThan(
      producerWorkflow.indexOf('| node scripts/windows-update-native-chain.mjs key-import'),
    );
    expect(producerWorkflow).toContain('key-import --key-path $key --descriptor $descriptor');
    expect(producerWorkflow).toContain('force_producer_failure');
    expect(acceptanceProducer).toContain('Forced installed-acceptance producer build failure');
    expect(acceptanceProducer.indexOf('await runStage(stage')).toBeLessThan(
      acceptanceProducer.indexOf('TQ_ACCEPTANCE_E2E_FORCE_BUILD_FAILURE'),
    );
    expect(acceptanceProducer).toContain("stage === 'native-signer'");
    expect(producerWorkflow).toContain(
      "TQ_ACCEPTANCE_E2E_FORCE_BUILD_FAILURE: ${{ inputs.force_producer_failure && '1' || '0' }}",
    );
    const importIndex = producerWorkflow.indexOf(
      '- name: Import protected updater key through stdin',
    );
    const cleanupIndex = producerWorkflow.indexOf('- name: Delete every protected producer key');
    const uploadIndex = producerWorkflow.indexOf(
      '- name: Upload cleaned protected producer result',
    );
    expect(importIndex).toBeGreaterThan(-1);
    expect(cleanupIndex).toBeGreaterThan(importIndex);
    expect(uploadIndex).toBeGreaterThan(cleanupIndex);
    expect(producerWorkflow.slice(importIndex, cleanupIndex)).not.toContain('uses:');
    const cleanup = producerWorkflow.slice(cleanupIndex, uploadIndex);
    expect(cleanup).toContain('if: always()');
    expect(cleanup).toContain('key-delete --descriptor $descriptor');
    expect(cleanup).not.toContain('cargo');
    expect(cleanup).not.toContain('key-delete --key-path');
    const upload = producerWorkflow.slice(uploadIndex);
    expect(upload).toContain('if: success()');
    expect(upload).toContain('actions/upload-artifact@');
    expect(producerWorkflow).toContain('GITHUB_STEP_SUMMARY');
    expect(producerWorkflow).toContain(
      'node scripts/windows-update-public-key.mjs build/windows-update-public-key.sec1',
    );
    expect(producerWorkflow).not.toContain(
      'Get-FileHash -LiteralPath build/windows-update-public-key.sec1',
    );
  });

  it('hands the reboot to Windows and binds accepted shutdown evidence', () => {
    expect(realRebootWorkflow).toContain("Join-Path $env:SystemRoot 'System32\\shutdown.exe'");
    expect(realRebootWorkflow).toContain('& $shutdown /r /t 30');
    expect(realRebootWorkflow).toContain('if ($shutdownExit -ne 0)');
    expect(realRebootWorkflow).toContain("rebootRequestMethod = 'shutdown.exe'");
    expect(realRebootWorkflow).toContain('rebootRequestAcceptedAt');
    expect(realRebootWorkflow).toContain(
      'Pending deletion source disappeared immediately before shutdown.exe',
    );
    expect(realRebootWorkflow).not.toContain('Start-Sleep -Seconds 30');
    expect(realRebootWorkflow).not.toContain('Restart-Computer');
    expect(realRebootWorkflow).not.toContain('RUNNER_TRACKING_ID');
    expect(realRebootWorkflow).toContain("shutdown.exe') /a");
    expect(realRebootWorkflow).toContain('if: failure()');
    expect(realRebootWorkflow).toContain('if: always()');
    expect(realRebootWorkflow).toContain('Post-reboot acceptance cleanup failed');
    expect(realRebootWorkflow).toContain('<Interval>PT15M</Interval>');
    expect(realRebootWorkflow).toContain('/Create /F /TN $taskName /XML $taskXmlPath');
    expect(realRebootWorkflow).toContain('Watchdog retained protected residue');
    expect(realRebootWorkflow).toContain(
      'TalkingQuillRealRebootAcceptance-${{ inputs.architecture }}',
    );
    expect(realRebootWorkflow).toContain(
      'Remove-Item -LiteralPath $env:CHECKPOINT,"$env:CHECKPOINT.reboot-request.json"',
    );
  });

  it('requires native smoke and exact staged provenance before ordinary assembly', () => {
    const assemble = section('assemble');
    expect(assemble).toContain('needs: [validate, package, smoke]');
    expect(assemble).toContain('assemble-ordinary-unsigned-release.mjs');
    expect(assemble).toContain('node scripts/release-checksums.mjs');
    expect(workflow).not.toContain('migration-lifecycle');
    expect(workflow).not.toContain('windows-terminal-fault-artifacts');
    const smoke = section('smoke', 'assemble');
    expect(smoke).toContain('runner: windows-11-arm');
    expect(smoke).toContain('runner: windows-latest');
    expect(smoke).toContain('windows-hosted-lifecycle-smoke.ps1');
    expect(smoke).toContain('windows-hosted-lifecycle-${{ matrix.arch }}.json');
  });
});
