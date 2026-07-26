import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('reproducible security gate policy', () => {
  it('pins external actions, fetches full history, and uses frozen installs in every gate', async () => {
    for (const path of [
      '.github/workflows/ci.yml',
      '.github/workflows/packaged-gate.yml',
      '.github/workflows/packaged-smoke.yml',
      '.github/workflows/release.yml',
    ]) {
      const workflow = await readFile(path, 'utf8');
      expect(workflow).not.toMatch(/uses:\s+[^\s]+@v\d/u);
      expect(workflow).toContain('fetch-depth: 0');
      expect(workflow).toContain('pnpm install --frozen-lockfile');
      expect(workflow).toContain('node scripts/ensure-cargo-audit.mjs');
      expect(workflow).toContain('pnpm security:gate');
      if (path.includes('packaged-')) {
        expect(workflow).toContain("TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1'");
        expect(workflow).toContain('TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED:');
        expect(workflow).toContain('TALKING_QUILL_PACKAGE_TARGET: ${{ matrix.target }}');
        expect(workflow).toContain('TALKING_QUILL_PACKAGE_ARCH: ${{ matrix.arch }}');
      }
    }
  });

  it('separates trusted candidate control, unsigned builds, signing, attestation, and publication', async () => {
    const [candidate, publication, trustVerifier] = await Promise.all([
      readFile('.github/workflows/release.yml', 'utf8'),
      readFile('.github/workflows/publish-release.yml', 'utf8'),
      readFile('scripts/verify-trusted-candidate.mjs', 'utf8'),
    ]);
    expect(candidate).toContain('workflow_dispatch:');
    expect(candidate).not.toContain("tags: ['v*.*.*']");
    expect(candidate.indexOf('external-controls:')).toBeLessThan(candidate.indexOf('preflight:'));
    expect(candidate.indexOf('unsigned-package:')).toBeLessThan(candidate.indexOf('\n  package:'));
    expect(candidate).toContain('environment: release-signing');
    expect(candidate).toContain(
      'actions/attest-build-provenance@e8998f949152b193b063cb0ec769d69d929409be',
    );
    expect(candidate).toContain("TALKING_QUILL_DETERMINISTIC: ${{ inputs.dry_run && '1' || '0' }}");
    expect(candidate).toContain('Reject candidate modifications after validation');
    expect(candidate).not.toMatch(/uses:\s+[^\s]+@v\d/u);
    for (const workflow of [candidate, publication]) {
      const checkoutCount = workflow.match(/actions\/checkout@/gu)?.length ?? 0;
      expect(checkoutCount).toBeGreaterThan(0);
      expect(workflow.match(/persist-credentials: false/gu)).toHaveLength(checkoutCount);
    }
    for (const [workflow, jobs] of [
      [
        candidate,
        [
          'external-controls',
          'preflight',
          'unsigned-package',
          'package',
          'assemble',
          'attest',
          'publish-draft',
        ],
      ],
      [
        publication,
        ['external-controls', 'dry-run-publication', 'verify-platform-signatures', 'publish'],
      ],
    ] as const) {
      for (const [index, job] of jobs.entries()) {
        const start = workflow.indexOf(`\n  ${job}:`);
        const nextJob = jobs[index + 1];
        const end =
          nextJob === undefined ? workflow.length : workflow.indexOf(`\n  ${nextJob}:`, start);
        expect(workflow.slice(start, end), job).toContain('timeout-minutes:');
      }
    }
    expect(publication).toContain('environment: release-publication');
    expect(publication).toContain('gh attestation verify "$asset"');
    expect(publication).toContain('tmp/publication/redownload');
    expect(publication).toContain('tmp/publication/public-download');
    expect(publication).toContain('cmp "$asset"');
    expect(publication).toContain('scripts/verify-public-release.mjs');
    expect(publication).toContain('RELEASE_EXTERNAL_BLOCKERS_APPROVAL');
    expect(publication).toContain('If-Match: $etag');
    expect(publication).toContain('make_latest');
    expect(publication.indexOf('verify-draft-release.mjs')).toBeLessThan(
      publication.indexOf('--request PATCH'),
    );
    for (const protectedValue of [
      'RELEASE_TAG_SIGNING_PUBLIC_KEY',
      'RELEASE_TAG_SIGNER_FINGERPRINTS',
      'RELEASE_POLICY_TREE_SHA256',
    ]) {
      expect(candidate).toContain(protectedValue);
      expect(publication).toContain(protectedValue);
    }
    expect(trustVerifier).toContain("['rev-parse', `origin/${defaultBranch}^{commit}`]");
    expect(trustVerifier).toContain('must equal the protected default-branch head');
    expect(candidate).toContain('--prepackaged');
    expect(candidate).toContain("NODE_OPTIONS: ''");
    expect(
      candidate.indexOf(
        'node scripts/artifact-provenance.mjs --verify',
        candidate.indexOf('\n  package:'),
      ),
    ).toBeLessThan(
      candidate.indexOf('Verify Windows Authenticode chain', candidate.indexOf('\n  package:')),
    );
    expect(trustVerifier).toContain("['status', '--porcelain=v1', '--untracked-files=normal']");
    expect(trustVerifier).toContain("['verify-tag', '--raw', rawTag]");
  });

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

  it('wires exact final-artifact requirements for package and directory-only gates', async () => {
    const [releaseWorkflow, directoryWorkflow, { createPackagePlan }] = await Promise.all([
      readFile('.github/workflows/packaged-smoke.yml', 'utf8'),
      readFile('.github/workflows/packaged-gate.yml', 'utf8'),
      import('../../scripts/run-package.mjs'),
    ]);
    expect(releaseWorkflow.match(/artifactRequirement: nsis/gu)).toHaveLength(2);
    expect(releaseWorkflow.match(/artifactRequirement: dmg-zip/gu)).toHaveLength(2);
    expect(releaseWorkflow).toContain(
      'TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: ${{ matrix.artifactRequirement }}',
    );
    expect(releaseWorkflow).toContain('id: inspect-package');
    expect(releaseWorkflow).toContain('path: ${{ steps.inspect-package.outputs.artifact_paths }}');
    expect(releaseWorkflow).not.toContain('Talking-Quill-*');
    expect(releaseWorkflow).not.toContain('path: release/*');
    expect(directoryWorkflow).toContain('TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: none');
    expect(createPackagePlan('win-dir')).toMatchObject({
      artifactRequirement: 'none',
      platform: 'win',
      architecture: 'x64',
    });
    expect(createPackagePlan('win-arm64')).toMatchObject({
      artifactRequirement: 'nsis',
      platform: 'win',
      architecture: 'arm64',
    });
    expect(createPackagePlan('mac-arm64')).toMatchObject({
      artifactRequirement: 'dmg-zip',
      platform: 'mac',
      architecture: 'arm64',
    });
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
      packageManager: 'pnpm@11.13.0',
      scripts: {
        'security:release-gate': 'pnpm install --frozen-lockfile && pnpm security:gate',
      },
    });
  });

  it('pins the supplemental gitleaks binary and forces an all-history exact config', async () => {
    const [runner, config, workflow] = await Promise.all([
      readFile('scripts/run-gitleaks.ps1', 'utf8'),
      readFile('.gitleaks.toml', 'utf8'),
      readFile('.github/workflows/ci.yml', 'utf8'),
    ]);
    expect(runner).toContain("$Version = '8.28.0'");
    expect(runner).toContain(
      "$ExpectedSha256 = 'DA6458E8864AF553807DE1C46A7A8EAC0880BD6B99BA56288E87E86A45AF884F'",
    );
    expect(runner).toContain("--log-opts='--all'");
    expect(runner).toContain('Remove-Item -Path $Executable');
    expect(runner.indexOf('Get-FileHash $Archive')).toBeLessThan(
      runner.indexOf('Expand-Archive -Path $Archive'),
    );
    expect(workflow).toContain('run: ./scripts/run-gitleaks.ps1');
    expect(config).not.toMatch(/paths\s*=|commits\s*=/u);
    expect(config).toContain(['^sk', 'myApiKeyToAccessMyChromaInstance$'].join('-'));
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
