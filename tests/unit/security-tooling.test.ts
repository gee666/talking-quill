import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('reproducible security tooling policy', () => {
  it('wires least-privilege per-file macOS signing and fail-closed entitlement checks', async () => {
    const [builder, signer, verifier, manifest] = await Promise.all([
      readFile('build/electron-builder.yml', 'utf8'),
      readFile('build/mac-sign.cjs', 'utf8'),
      readFile('scripts/verify-macos-release.mjs', 'utf8'),
      readFile('package.json', 'utf8'),
    ]);
    expect(builder).toContain('sign: ../build/mac-sign.cjs');
    expect(signer).toContain('entitlements: []');
    expect(signer).toContain(
      "RUST_HELPER_RELATIVE_PATH = 'Contents/Resources/helper/talking-quill-helper'",
    );
    expect(verifier).toContain('Entitlement extraction failed');
    expect(verifier).not.toContain('if (result.status !== 0) return {}');
    const parsedManifest = JSON.parse(manifest) as {
      devDependencies: Record<string, string>;
    };
    expect(parsedManifest.devDependencies).toMatchObject({ '@electron/osx-sign': '1.3.3' });
  });

  it('keeps dependency policy explicit and forbids advisory-ignore flags', async () => {
    const [nodeAudit, rustAudit, manifest] = await Promise.all([
      readFile('scripts/audit-node-production.mjs', 'utf8'),
      readFile('scripts/audit-rust.mjs', 'utf8'),
      readFile('package.json', 'utf8'),
    ]);
    expect(nodeAudit).toContain("'audit', '--prod', '--json'");
    expect(nodeAudit).toContain('counts.high + counts.critical');
    expect(rustAudit).toContain("'--deny', 'warnings'");
    expect(rustAudit).not.toContain("'--ignore'");
    const { CARGO_AUDIT_VERSION, isExpectedCargoAuditVersion } =
      await import('../../scripts/security-tool-versions.mjs');
    expect(CARGO_AUDIT_VERSION).toBe('0.22.2');
    expect(isExpectedCargoAuditVersion('cargo-audit-audit 0.22.2\n')).toBe(true);
    expect(isExpectedCargoAuditVersion('cargo-audit 0.22.2')).toBe(false);
    expect(isExpectedCargoAuditVersion('not-cargo-audit 0.22.2')).toBe(false);
    expect(isExpectedCargoAuditVersion('cargo-audit 1.0.22.2')).toBe(false);
    expect(JSON.parse(manifest)).toMatchObject({
      packageManager: 'pnpm@11.17.0',
      scripts: {
        'security:release-gate': 'pnpm install --frozen-lockfile && pnpm security:gate',
      },
    });
  });

  it('binds every historical secret advisory to exact immutable evidence', async () => {
    const { validateSecretHistoryAllowlist } =
      await import('../../scripts/secret-history-schema.mjs');
    const allowlist = JSON.parse(
      await readFile('scripts/secret-history-allowlist.json', 'utf8'),
    ) as { schemaVersion: number; entries: Record<string, unknown>[] };
    expect(() => validateSecretHistoryAllowlist(allowlist)).not.toThrow();
    expect(allowlist.schemaVersion).toBe(2);
    expect(allowlist.entries).toHaveLength(2);
    expect(allowlist.entries[0]).toMatchObject({
      path: 'scripts/secret-rules.mjs',
      classification: 'reviewed-nonsecret',
    });
    expect(
      new Set(allowlist.entries.map(({ blobOid, rule }) => `${String(blobOid)}:${String(rule)}`))
        .size,
    ).toBe(2);

    const extra = structuredClone(allowlist);
    const first = extra.entries[0];
    if (first === undefined) throw new Error('Expected allowlist fixture');
    first.unexpected = true;
    expect(() => validateSecretHistoryAllowlist(extra)).toThrow(/schema/u);
    const duplicate = structuredClone(allowlist);
    const duplicateEntry = duplicate.entries[0];
    if (duplicateEntry === undefined) throw new Error('Expected allowlist fixture');
    duplicate.entries.push(structuredClone(duplicateEntry));
    expect(() => validateSecretHistoryAllowlist(duplicate)).toThrow(/Duplicate/u);
  });
});
