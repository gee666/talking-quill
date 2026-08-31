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
    expect(workflow).toContain('Talking-Quill-$version-win-$arch-update.exe');
    expect(workflow).toContain('for arch in x64 arm64');
    expect(workflow).not.toContain('mac-x64');
    expect(workflow).toContain('release-identity-win-${arch}.json');
    expect(workflow).toContain('provenance-win-$arch-setup.json');
    expect(workflow).toContain('provenance-win-$arch-update.json');
    expect(workflow).toContain('talkingQuillRelease:');
    expect(workflow).toContain('gateway,owner');
    expect(workflow).toContain('--draft');
    expect(draftVerifier).toContain('manifest.promotable !== true');
    expect(workflow).not.toMatch(/^\s+--prerelease\s*$/mu);
    const verify = workflow.indexOf('node scripts/verify-draft-release.mjs');
    const promote = workflow.indexOf('curl --fail-with-body --silent --show-error --request PATCH');
    expect(verify).toBeGreaterThan(-1);
    expect(promote).toBeGreaterThan(verify);
    expect(workflow).toContain('-H "If-Match: $etag"');
    expect(workflow).toContain('"draft":false,"prerelease":false,"make_latest":"true"');
    expect(workflow).not.toContain('gh release edit');
    const freshFetch = workflow.indexOf(
      'https://api.github.com/repos/$REPOSITORY/releases/$RELEASE_ID',
    );
    const inventoryCheck = workflow.indexOf('Fresh promotion release metadata differs');
    expect(freshFetch).toBeGreaterThan(verify);
    expect(inventoryCheck).toBeGreaterThan(freshFetch);
    expect(promote).toBeGreaterThan(inventoryCheck);
  });

  it('leaves promotion as the final command so every earlier failure preserves the draft', () => {
    const executableLines = workflow
      .split(/\r?\n/u)
      .map((line) => line.trim())
      .filter((line) => line !== '' && !line.startsWith('#'));
    expect(executableLines.at(-1)).toContain(
      'curl --fail-with-body --silent --show-error --request PATCH',
    );
    const promote = workflow.indexOf('curl --fail-with-body --silent --show-error --request PATCH');
    expect(workflow.slice(promote).match(/\bcurl\s/gu)).toHaveLength(1);
    expect(workflow.slice(promote)).toContain(
      'Published release response identity or asset metadata mismatch',
    );
    expect(workflow.indexOf('fresh.draft')).toBeLessThan(promote);
    expect(workflow.indexOf('fresh.assets')).toBeLessThan(promote);
    expect(workflow.indexOf('release_id="$(jq -er')).toBeLessThan(promote);
    expect(workflow).toContain('RELEASE_ID: ${{ steps.verified_draft.outputs.release_id }}');
    expect(workflow).toContain('releases/assets/$asset_id');
    expect(workflow).not.toContain('gh release download');
    expect(workflow.slice(promote)).not.toMatch(/verify|download|fresh\.assets|fresh\.draft/iu);
  });

  it('keeps the assembled producer and draft consumer inventories identical', () => {
    expect(producer).toContain('node scripts/assemble-release.mjs');
    expect(producer).toContain('for arch in x64 arm64');
    expect(workflow).toContain('for arch in x64 arm64');
    for (const name of [
      'latest-$arch.yml',
      'release-identity-win-$arch.json',
      'provenance-win-$arch-setup.json',
      'provenance-win-$arch-update.json',
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
    expect(workflow).toContain('Talking-Quill-$version-win-$arch-update.exe');
    expect(workflow).toContain('Talking-Quill-$version-win-$arch-update.exe.blockmap');
    expect(workflow).toContain('SHA256SUMS.txt');
    expect(draftVerifier).toContain('SHA256SUMS.txt');
  });
});
