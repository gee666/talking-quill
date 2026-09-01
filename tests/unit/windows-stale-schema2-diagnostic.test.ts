import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const source = readFileSync('installer/windows-setup/src/windows.rs', 'utf8');
const rustPackage = readFileSync('installer/windows-setup/src/package.rs', 'utf8');
const productionBuild = readFileSync('scripts/build-windows-setup.mjs', 'utf8');
const cleanupBuild = readFileSync('scripts/build-windows-stale-schema2-cleanup-setup.mjs', 'utf8');
const pack = readFileSync('scripts/pack-windows-native.mjs', 'utf8');
const e2e = readFileSync('scripts/run-windows-stale-schema2-diagnostic-e2e.mjs', 'utf8');

describe('packaged stale schema-2 diagnosis', () => {
  it('gates the exact noninteractive command with the cleanup feature', () => {
    expect(source).toContain('fn direct_diagnostic_arguments(arguments: &[OsString]) -> bool');
    expect(source).toContain(
      'arguments.len() == 1 && arguments[0] == "/TQ-DIAGNOSE-STALE-SCHEMA2"',
    );
    const dispatch = source.indexOf('if direct_diagnostic_arguments(&arguments) {');
    expect(dispatch).toBeGreaterThan(-1);
    expect(dispatch).toBeLessThan(source.indexOf('let elevated = token_is_elevated()?;'));
    expect(
      source.indexOf('return run_direct_stale_schema2_diagnostic(&arguments)', dispatch),
    ).toBeGreaterThan(dispatch);
  });

  it('uses stable stages and append-only protected evidence', () => {
    for (const stage of [
      'request.exact-argv',
      'token.identity',
      'self.path',
      'self.open',
      'self.acl',
      'self.identity',
      'self.sha256',
      'package.tqpkg2',
      'package.source-binding',
      'audit.environment',
      'audit.path',
      'audit.acl',
      'audit.open',
      'audit.initialize',
      'audit.event',
      'mutex.availability',
      'paths.known-folders',
      'lifecycle-lock.availability',
      'registry.inventory',
      'active-state.inventory',
      'fixture.identity',
      'fixture.sha256',
      'image.stability',
      'diagnostic.complete',
      'cleanup.rejected.before-audit',
      'cleanup.rejected.after-audit',
    ]) {
      expect(source).toContain(`"${stage}"`);
    }
    expect(source).toContain('TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH is required.');
    expect(source).toContain('.append(true)');
    expect(source).toContain('FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH');
    expect(source).toContain('self.file.sync_all().map_err(io_failure)?;');
  });

  it('wraps each diagnostic operation with its exact stage', () => {
    const diagnostic = source.slice(
      source.indexOf('fn run_direct_stale_schema2_diagnostic_inner('),
      source.indexOf('fn force_stale_cleanup_rejection('),
    );
    expect(diagnostic).not.toContain('diagnostic.record(');
    expect(diagnostic).not.toContain('diagnostic.rejected');
    for (const stage of [
      'token.identity',
      'self.path',
      'self.open',
      'self.acl',
      'self.identity',
      'self.sha256',
      'package.tqpkg2',
      'package.source-binding',
      'audit.environment',
      'audit.path',
      'audit.open',
      'audit.acl',
      'audit.initialize',
      'mutex.availability',
      'paths.known-folders',
      'registry.inventory',
      'active-state.inventory',
      'image.stability',
    ]) {
      expect(diagnostic).toContain(`diagnostic_stage(diagnostic, "${stage}", ||`);
    }
  });

  it('publishes audit completion only after final image stability', () => {
    const diagnostic = source.slice(
      source.indexOf('fn run_direct_stale_schema2_diagnostic_inner('),
      source.indexOf('fn force_stale_cleanup_rejection('),
    );
    const stability = [...diagnostic.matchAll(/"image\.stability"/gu)].map((match) => match.index);
    const auditCompletion = [...diagnostic.matchAll(/audit\.record\("diagnostic-complete"/gu)].map(
      (match) => match.index,
    );
    const terminal = [...diagnostic.matchAll(/"diagnostic\.complete"/gu)].map(
      (match) => match.index,
    );
    expect(stability).toHaveLength(2);
    expect(auditCompletion).toHaveLength(2);
    expect(terminal).toHaveLength(2);
    for (let index = 0; index < 2; index += 1) {
      expect(stability[index]).toBeLessThan(auditCompletion[index] ?? -1);
      expect(auditCompletion[index]).toBeLessThan(terminal[index] ?? -1);
    }
  });

  it('keeps diagnosis out of mutation APIs', () => {
    const diagnostic = source.slice(
      source.indexOf('fn run_direct_stale_schema2_diagnostic('),
      source.indexOf('fn run_direct_elevated_stale_schema2_cleanup()'),
    );
    for (const mutation of [
      'apply_lock_dacl(',
      'protect_stale_registry_key(',
      '.delete()',
      '.rename(',
      'delete_registry_tree_durable(',
    ]) {
      expect(diagnostic).not.toContain(mutation);
    }
  });

  it('isolates and marks the nonpromotable cleanup package', () => {
    expect(cleanupBuild).toContain("'windows-setup-stale-schema2-cleanup'");
    expect(cleanupBuild).toContain("'stale-schema2-cleanup'");
    expect(cleanupBuild).toContain('staleSchema2Cleanup: true');
    expect(productionBuild).toContain("'windows-setup-production'");
    expect(productionBuild).toContain("Buffer.from(marker, 'utf16le')");
    expect(pack).toContain("packageMode === 'stale-schema2-cleanup'");
    expect(rustPackage).toContain('cfg!(feature = "stale-schema2-cleanup")');
  });

  it('rebuilds and runs current-source TQPKG2 diagnostics with immutable snapshots', () => {
    expect(e2e).toContain("git(['rev-parse', 'HEAD'])");
    expect(e2e).toContain("git(['rev-parse', 'HEAD^{tree}'])");
    expect(e2e).toContain("git(['status', '--porcelain', '--untracked-files=no'])");
    expect(e2e).toContain('await rebuildCurrentArtifacts();');
    expect(e2e).toContain('rm(cleanupTarget, { recursive: true, force: true })');
    expect(e2e).toContain('canonicalPackage.manifest.sourceCommit !== sourceCommit');
    expect(e2e).toContain('diagnosticPackage.manifest.sourceTree !== sourceTree');
    expect(e2e).toContain('parseTqpkg2(await readFile(canonical)');
    expect(e2e).toContain('allowStaleSchema2Cleanup: true');
    expect(e2e).toContain('canonicalResult.status !== 64');
    expect(e2e).toContain('rejectedDispatch.status !== 64');
    expect(e2e).toContain('expectedRejectionStage === undefined ? [0] : [78]');
    expect(e2e).toContain('last?.stageCode !== expectedRejectionStage');
    expect(e2e).toContain('JSON.stringify(before.immutable) !== JSON.stringify(after.immutable)');
    expect(e2e).toContain('verifyDiagnosticChain');
    expect(e2e).toContain('verifyAuditChain');
  });
});
