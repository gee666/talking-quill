import { describe, expect, it } from 'vitest';
import { validateExternalControls } from '../../scripts/release-control-preflight.mjs';
import {
  parseApprovedFingerprints,
  POLICY_PATHS,
} from '../../scripts/verify-trusted-candidate.mjs';
import { validatePublicRelease } from '../../scripts/verify-public-release.mjs';

const policy = {
  repository: 'gee666/talking-quill',
  defaultBranch: 'main',
  environments: ['release-trust', 'release-signing', 'release-publication'],
  runnerLabels: ['release-runner'],
  environmentSecrets: { 'release-signing': ['SIGNING_KEY'] },
  environmentVariables: { 'release-publication': ['APPROVAL'] },
};

function environment() {
  return {
    protection_rules: [
      {
        type: 'required_reviewers',
        reviewers: [{ type: 'User', id: 1 }],
        prevent_self_review: true,
      },
    ],
    deployment_branch_policy: { protected_branches: true, custom_branch_policies: false },
  };
}

describe('release control-plane policy', () => {
  it('requires the canonical protected default branch and every reviewer-protected environment', () => {
    const controls = {
      repository: {
        full_name: policy.repository,
        default_branch: policy.defaultBranch,
        defaultBranchProtected: true,
        visibility: 'public',
        private: false,
        immutable_releases: true,
      },
      environments: Object.fromEntries(policy.environments.map((name) => [name, environment()])),
      tagRulesets: [{ target: 'tag', enforcement: 'active', rules: [{ type: 'creation' }] }],
      runnerLabels: ['release-runner'],
      environmentSecrets: { 'release-signing': ['SIGNING_KEY'] },
      environmentVariables: { 'release-publication': ['APPROVAL'] },
    };
    expect(() => validateExternalControls(controls, policy)).not.toThrow();
    expect(() =>
      validateExternalControls(
        { ...controls, repository: { ...controls.repository, defaultBranchProtected: false } },
        policy,
      ),
    ).toThrow('not protected');
    expect(() =>
      validateExternalControls(
        {
          ...controls,
          environments: {
            ...controls.environments,
            'release-publication': {
              ...environment(),
              protection_rules: [],
            },
          },
        },
        policy,
      ),
    ).toThrow('no required reviewer');
    expect(() =>
      validateExternalControls(
        { ...controls, repository: { ...controls.repository, immutable_releases: false } },
        policy,
      ),
    ).toThrow('Immutable releases');
    expect(() => validateExternalControls({ ...controls, runnerLabels: [] }, policy)).toThrow(
      'runner label',
    );
    expect(() => validateExternalControls({ ...controls, environmentSecrets: {} }, policy)).toThrow(
      'secret is missing',
    );
  });

  it('keeps signer and policy controls closed and explicit', () => {
    expect(parseApprovedFingerprints(` ${'A'.repeat(40)},${'b'.repeat(64)} `)).toEqual(
      new Set(['A'.repeat(40), 'B'.repeat(64)]),
    );
    expect(() => parseApprovedFingerprints('')).toThrow();
    expect(() => parseApprovedFingerprints('not-a-fingerprint')).toThrow();
    expect(POLICY_PATHS).toContain('.github/workflows/publish-release.yml');
    expect(POLICY_PATHS).toContain('scripts/verify-public-release.mjs');
  });

  it('accepts public latest only when it exactly matches the published tag response', () => {
    const assetNames = [
      'Talking-Quill-1.0.0-win-x64.exe',
      'release-manifest.json',
      'SHA256SUMS.txt',
    ];
    const expected = {
      draft: false,
      prerelease: false,
      tag_name: 'v1.0.0',
      target_commitish: 'a'.repeat(40),
      html_url: 'https://github.com/gee666/talking-quill/releases/tag/v1.0.0',
      assets: assetNames.map((name) => ({ name })),
    };
    const identity = { tag: 'v1.0.0', commit: 'a'.repeat(40), assetNames };
    expect(() =>
      validatePublicRelease(expected, structuredClone(expected), identity),
    ).not.toThrow();
    expect(() =>
      validatePublicRelease(expected, { ...structuredClone(expected), draft: true }, identity),
    ).toThrow('Public latest');
    expect(() =>
      validatePublicRelease(
        expected,
        { ...structuredClone(expected), assets: expected.assets.slice(1) },
        identity,
      ),
    ).toThrow('Public latest');
  });
});
