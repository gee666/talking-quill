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
    expect(source).toContain('return run_direct_stale_schema2_diagnostic(&arguments)');
    expect(source.indexOf('if direct_diagnostic_requested {')).toBeLessThan(
      source.indexOf('if !elevated {'),
    );
  });

  it('uses stable stages and append-only protected evidence', () => {
    for (const stage of [
      'request.exact-argv',
      'token.identity',
      'self.path',
      'self.acl',
      'self.identity',
      'self.sha256',
      'package.tqpkg2',
      'package.source-binding',
      'audit.environment',
      'audit.path',
      'audit.acl',
      'audit.open',
      'mutex.availability',
      'lifecycle-lock.availability',
      'registry.inventory',
      'active-state.inventory',
      'fixture.identity',
      'fixture.sha256',
      'diagnostic.complete',
      'diagnostic.rejected',
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

  it('tests final TQPKG2 executables and exit contracts', () => {
    expect(e2e).toContain('parseTqpkg2(await readFile(canonical)');
    expect(e2e).toContain('allowStaleSchema2Cleanup: true');
    expect(e2e).toContain('canonicalResult.status !== 64');
    expect(e2e).toContain('diagnosticResult.status !== 78');
  });
});
