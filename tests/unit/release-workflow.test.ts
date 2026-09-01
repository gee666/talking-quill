import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const publishWorkflow = readFileSync('.github/workflows/publish-local-owner.yml', 'utf8');
const realRebootWorkflow = readFileSync(
  '.github/workflows/windows-real-reboot-acceptance.yml',
  'utf8',
);
const stageScript = readFileSync('scripts/stage-unsigned-release.mjs', 'utf8');
const assembleScript = readFileSync('scripts/assemble-release.mjs', 'utf8');
const promotionScript = readFileSync('scripts/windows-promotion-evidence.mjs', 'utf8');

function section(start: string, end?: string): string {
  const startIndex = workflow.indexOf(`\n  ${start}:`);
  const endIndex = end === undefined ? workflow.length : workflow.indexOf(`\n  ${end}:`);
  expect(startIndex, start).toBeGreaterThan(-1);
  expect(endIndex, end).toBeGreaterThan(startIndex);
  return workflow.slice(startIndex, endIndex);
}

describe('Windows native release workflow', () => {
  it('runs validation and builds only the 0.0.69 fresh trust-root package', () => {
    const validate = section('validate', 'package');
    const packageJob = section('package', 'smoke');
    expect(validate).toContain('pnpm validate:unsigned-release');
    expect(validate).toContain('pnpm security:gate');
    expect(packageJob).toContain('package_script: package:win');
    expect(packageJob).toContain('package_script: package:win:arm64');
    expect(packageJob).toContain('TALKING_QUILL_PACKAGE_MODE: fresh');
    expect(packageJob).toContain("TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT: '1'");
    expect(packageJob).not.toContain('Build explicit predecessor-bound update');
    expect(workflow).not.toContain('predecessor_x64_');
    expect(workflow).not.toContain('predecessor_arm64_');
    expect(workflow).not.toContain('PREDECESSOR_INSTALLER_URL');
    expect(stageScript).toContain('windowsFreshTrustRoot');
    expect(assembleScript).toContain('freshTrustRoot');
    expect(assembleScript).toContain('fresh trust-root provenance identity mismatch');
    expect(packageJob).not.toContain('latest-${{ matrix.arch }}.yml');
    expect(packageJob).not.toContain('release-identity-win-${{ matrix.arch }}.json');
  });

  it('uses an actual same-repository local baseline artifact and proves preservation', () => {
    const migration = section('migration-lifecycle', 'fresh-lifecycle');
    expect(migration).toContain('environment: windows-local-migration-trust');
    expect(migration).toContain('TALKING_QUILL_LOCAL_0067_X64_BASELINE_RUN_ID');
    expect(migration).toContain('TALKING_QUILL_LOCAL_0067_ARM64_BASELINE_RUN_ID');
    expect(migration).toContain(
      "gh run download $env:BASELINE_RUN_ID --repo '${{ github.repository }}'",
    );
    expect(migration).toContain(
      "$run.path -cne '.github/workflows/windows-local-0067-baseline.yml'",
    );
    expect(migration).toContain('BASELINE_ARTIFACT_DIGEST');
    expect(migration).toContain('BASELINE_MANIFEST_SHA256');
    expect(migration).not.toContain('Invoke-WebRequest');
    expect(migration).toContain("mode='local-uninstall-preserve-fresh'");
    expect(migration).toContain("provenance='local-non-public'");
    expect(migration).toContain('installedManifestUtf8Base64');
    expect(migration).toContain('installedManifest=$installedManifest');
    expect(migration).toContain('installedReleaseBuildDigest');
    expect(migration).toContain('installedGatewaySha256');
    expect(migration).toContain('installedOwnerSha256');
    expect(migration).toContain('updaterMarkerPresent=$false');
    expect(migration).toContain('profileInventoryAfterUninstall');
    expect(migration).toContain('modelInventoryAfterFresh');
    expect(migration).toContain('sentinelInventoryAfterUninstall');
    expect(migration).toContain('sentinelInventoryAfterFresh');
    expect(migration).toContain('machineQuitObserved');
    expect(migration).toContain('singletonReleased');
    expect(migration).toContain('uninstallResidue=@($machineResidue)');
    expect(migration).toContain('windows-${{ matrix.arch }}-local-migration-evidence');
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

  it('signs and publishes migration evidence while excluding updater and fault binaries', () => {
    const assemble = section('assemble');
    expect(assemble).toContain(
      'needs: [validate, package, smoke, migration-lifecycle, fresh-lifecycle]',
    );
    expect(assemble).toContain('windows-*-local-migration-evidence');
    expect(assemble).toContain('windows-terminal-fault-candidate-${process.env.ARCH}.json');
    expect(assemble).not.toContain('cp installed-evidence/*.json');
    expect(assemble).not.toContain('Talking-Quill-*-win-$arch-update.exe');
    expect(promotionScript).toContain("mode: 'fresh-trust-root'");
    expect(promotionScript).toContain("provenance: 'local-non-public'");
    expect(promotionScript).toContain("operation === 'local-uninstall-preserve-fresh'");
    expect(publishWorkflow).toContain('windows-local-migration-$arch.json');
    expect(publishWorkflow).toContain(
      'Fresh trust-root publication contains forbidden updater lineage assets.',
    );
    expect(workflow).toContain('Nonpromotable terminal acceptance setup entered release assembly.');
    expect(workflow).not.toMatch(/cp .*terminalAcceptance.* release-artifacts/u);
  });
});
