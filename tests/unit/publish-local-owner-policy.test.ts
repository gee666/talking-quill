import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/publish-local-owner.yml', 'utf8');
const producer = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const draftVerifier = readFileSync('scripts/verify-draft-release.mjs', 'utf8');

describe('Windows native publication policy', () => {
  it('publishes only the preserved checksum-verified x64 and ARM64 candidates', () => {
    expect(workflow).toContain('windows-native-release-candidates');
    expect(workflow).toContain('sha256sum --check SHA256SUMS.txt');
    expect(workflow).toContain('Talking-Quill-$version-win-$arch-setup.exe');
    expect(workflow).not.toContain('Talking-Quill-$version-win-$arch-update.exe');
    expect(workflow).toContain('for arch in x64 arm64');
    expect(workflow).not.toContain('mac-x64');
    expect(workflow).toContain('provenance-win-$arch-setup.json');
    expect(workflow).toContain('windows-local-migration-$arch.json');
    expect(workflow).toContain('windows-terminal-fault-candidate-$arch.json');
    expect(workflow).toContain(
      'Fresh trust-root publication contains forbidden updater lineage assets.',
    );
    expect(workflow).toContain('--draft');
    expect(draftVerifier).toContain('manifest.promotable !== true');
    expect(workflow).not.toMatch(/^\s+--prerelease\s*$/mu);
    const verify = workflow.indexOf('node scripts/verify-draft-release.mjs');
    const promote = workflow.indexOf('curl --fail-with-body --silent --show-error --request PATCH');
    expect(verify).toBeGreaterThan(-1);
    expect(promote).toBeGreaterThan(verify);
    expect(workflow).toContain('-H "If-Match: $etag"');
    expect(workflow).toContain('"draft":false,"prerelease":false,"make_latest":"legacy"');
    expect(workflow).not.toContain('"make_latest":"true"');
    expect(workflow).not.toContain('gh release edit');
    const freshFetch = workflow.indexOf(
      'https://api.github.com/repos/$REPOSITORY/releases/$RELEASE_ID',
    );
    const inventoryCheck = workflow.indexOf('Fresh promotion release metadata differs');
    expect(freshFetch).toBeGreaterThan(verify);
    expect(inventoryCheck).toBeGreaterThan(freshFetch);
    expect(promote).toBeGreaterThan(inventoryCheck);
  });

  it('serializes publication and rechecks all signed immutable history before promotion', () => {
    expect(workflow).toContain('group: publish-windows-owner\n');
    expect(workflow).not.toContain('group: publish-windows-owner-${{ inputs.tag }}');
    expect(workflow.match(/gh api --paginate --slurp/gu)).toHaveLength(4);
    expect(workflow.match(/releases\/\$id\/assets\?per_page=100/gu)).toHaveLength(2);
    expect(workflow.match(/node scripts\/publication-history\.mjs/gu)).toHaveLength(2);
    const draft = workflow.indexOf('gh release create');
    const promote = workflow.indexOf('curl --fail-with-body --silent --show-error --request PATCH');
    const historyChecks = [...workflow.matchAll(/node scripts\/publication-history\.mjs/gu)].map(
      ({ index }) => index,
    );
    expect(historyChecks[0]).toBeLessThan(draft);
    expect(historyChecks[1]).toBeGreaterThan(draft);
    expect(historyChecks[1]).toBeLessThan(promote);
    expect(workflow).toContain('select(.draft == false and .immutable == true)');
    expect(workflow).toContain('test "${#manifest_assets[@]}" -eq 1');
  });

  it('makes promotion the final mutation and then verifies immutable publication', () => {
    const promote = workflow.indexOf('curl --fail-with-body --silent --show-error --request PATCH');
    expect(workflow.slice(promote).match(/--request PATCH/gu)).toHaveLength(1);
    expect(workflow.slice(promote)).toContain(
      'Published release response identity or asset metadata mismatch',
    );
    expect(workflow.indexOf('fresh.draft')).toBeLessThan(promote);
    expect(workflow.indexOf('fresh.assets')).toBeLessThan(promote);
    expect(workflow.indexOf('release_id="$(jq -er')).toBeLessThan(promote);
    expect(workflow).toContain('RELEASE_ID: ${{ steps.verified_draft.outputs.release_id }}');
    expect(workflow).toContain('releases/assets/$asset_id');
    expect(workflow).not.toContain('gh release download');
    const immutableCheck = workflow.indexOf('Require immutable published release', promote);
    expect(immutableCheck).toBeGreaterThan(promote);
    expect(workflow.slice(immutableCheck)).toContain('gh release verify-asset');
    expect(workflow.slice(immutableCheck)).not.toMatch(/release (?:create|edit|upload)/u);
  });

  it('keeps the assembled producer and draft consumer inventories identical', () => {
    expect(producer).toContain('node scripts/assemble-release.mjs');
    expect(producer).toContain('for arch in x64 arm64');
    expect(workflow).toContain('for arch in x64 arm64');
    for (const name of [
      'provenance-win-$arch-setup.json',
      'windows-local-migration-$arch.json',
      'THIRD_PARTY_NOTICES.txt',
      'release-manifest.json',
      'SHA256SUMS.txt',
      'windows-promotion-lifecycle-evidence-v1.json',
    ]) {
      expect(producer, `producer ${name}`).toContain(name);
      expect(workflow, `consumer ${name}`).toContain(name);
    }
    expect(draftVerifier).toContain('...manifest.assets.map((asset) => asset.name)');
    expect(workflow).toContain('Talking-Quill-$version-win-$arch-setup.exe');
    expect(workflow).not.toContain('Talking-Quill-$version-win-$arch-update.exe');
    expect(workflow).not.toContain('Talking-Quill-$version-win-$arch-update.exe.blockmap');
    expect(workflow).toContain('SHA256SUMS.txt');
    expect(draftVerifier).toContain('SHA256SUMS.txt');
  });
});
