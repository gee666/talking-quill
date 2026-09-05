import { readFileSync, readdirSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/publish-local-owner.yml', 'utf8');

describe('ordinary unsigned publication policy', () => {
  it('grants write permission only to the publisher after authenticating the canonical producer', () => {
    const writers = readdirSync('.github/workflows').filter((name) =>
      /contents:\s*write/u.test(readFileSync(`.github/workflows/${name}`, 'utf8')),
    );
    expect(writers).toEqual(['publish-local-owner.yml']);
    expect(workflow).toContain('workflow_run:');
    expect(workflow).toContain('branches: [master]');
    expect(workflow).toContain("github.repository == 'gee666/talking-quill'");
    expect(workflow).toContain('test "$(jq -r .event <<<"$run")" = workflow_dispatch');
    expect(workflow).toContain('test "$(jq -r .conclusion <<<"$run")" = success');
    expect(workflow).toContain('test "$(jq -r .head_branch <<<"$run")" = master');
    expect(workflow).toContain('.github/workflows/release-unsigned.yml');
    expect(workflow).toContain('.head_repository.full_name');
    expect(workflow).toContain('value.workflowRunId !== process.env.RUN_ID');
    expect(workflow).toContain('value.sourceTree !== tree');
    expect(workflow.match(/commits\/master/gu)).toHaveLength(3);
    expect(workflow).not.toContain('secrets.');
    expect(workflow).not.toContain('environment:');
  });

  it('downloads only the successful run and byte-verifies a new draft before publishing', () => {
    expect(workflow).toContain('name: windows-native-release-candidates');
    expect(workflow).toContain('run-id: ${{ github.event.workflow_run.id }}');
    expect(workflow).toContain('group: publish-windows-owner');
    expect(workflow).toContain('Tag $TAG appeared');
    expect(workflow).toContain('Release $TAG appeared');
    expect(workflow).toContain('trap cleanup_draft EXIT');
    expect(workflow).toContain('sha256sum --check SHA256SUMS.txt');
    const verification = workflow.indexOf(
      'node scripts/verify-draft-release.mjs --ordinary-unsigned',
    );
    const publication = workflow.indexOf('gh release edit');
    expect(verification).toBeGreaterThan(-1);
    expect(publication).toBeGreaterThan(verification);
    expect(workflow.slice(publication)).toContain('--draft=false --prerelease=false --latest');
    expect(workflow.slice(publication)).toContain('node scripts/verify-public-release.mjs');
    expect(workflow.slice(publication)).toContain('refs/tags/$TAG^{commit}');
    expect(workflow).not.toContain('release-publication-manifest-v1.json');
    expect(workflow).toContain('evidence were not collected');
  });
});
