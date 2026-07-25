import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = resolve(import.meta.dirname, '..');

export function validateExternalControls(
  { repository, environments, tagRulesets, runnerLabels, environmentSecrets, environmentVariables },
  policy,
) {
  if (repository.full_name !== policy.repository) {
    throw new Error(`Canonical repository mismatch: ${String(repository.full_name)}`);
  }
  if (repository.default_branch !== policy.defaultBranch) {
    throw new Error(`Protected default branch mismatch: ${String(repository.default_branch)}`);
  }
  if (repository.archived === true || repository.disabled === true) {
    throw new Error('The canonical release repository is archived or disabled.');
  }
  if (repository.visibility !== 'public' || repository.private === true) {
    throw new Error('The canonical release repository is not publicly visible.');
  }
  if (repository.immutable_releases !== true) {
    throw new Error('Immutable releases are not enabled for the canonical repository.');
  }
  if (repository.defaultBranchProtected !== true) {
    throw new Error('The canonical default branch is not protected.');
  }
  if (
    !Array.isArray(tagRulesets) ||
    !tagRulesets.some(
      (ruleset) =>
        ruleset.target === 'tag' &&
        ruleset.enforcement === 'active' &&
        Array.isArray(ruleset.rules) &&
        ruleset.rules.some((rule) => rule.type === 'creation'),
    )
  ) {
    throw new Error('No active release-tag creation ruleset is configured.');
  }
  const availableLabels = new Set(runnerLabels);
  for (const label of policy.runnerLabels) {
    if (!availableLabels.has(label))
      throw new Error(`Required release runner label is absent: ${label}`);
  }
  for (const name of policy.environments) {
    const environment = environments[name];
    if (environment === undefined)
      throw new Error(`Required protected environment is missing: ${name}`);
    const reviewerRule = environment.protection_rules?.find(
      (rule) => rule.type === 'required_reviewers',
    );
    if (!Array.isArray(reviewerRule?.reviewers) || reviewerRule.reviewers.length === 0) {
      throw new Error(`Protected environment has no required reviewer: ${name}`);
    }
    if (reviewerRule.prevent_self_review !== true) {
      throw new Error(`Protected environment permits self-review: ${name}`);
    }
    if (environment.deployment_branch_policy?.protected_branches !== true) {
      throw new Error(`Protected environment does not require protected branches: ${name}`);
    }
    if (environment.deployment_branch_policy?.custom_branch_policies === true) {
      throw new Error(`Protected environment permits custom branch policies: ${name}`);
    }
    const secretNames = new Set(environmentSecrets[name] ?? []);
    for (const secret of policy.environmentSecrets[name] ?? []) {
      if (!secretNames.has(secret))
        throw new Error(`Protected environment secret is missing: ${name}:${secret}`);
    }
    const variableNames = new Set(environmentVariables[name] ?? []);
    for (const variable of policy.environmentVariables[name] ?? []) {
      if (!variableNames.has(variable))
        throw new Error(`Protected environment variable is missing: ${name}:${variable}`);
    }
  }
}

async function githubJson(path, token) {
  const response = await fetch(`https://api.github.com${path}`, {
    headers: {
      accept: 'application/vnd.github+json',
      authorization: `Bearer ${token}`,
      'user-agent': 'Talking-Quill-release-control/1',
      'x-github-api-version': '2022-11-28',
    },
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    throw new Error(`GitHub control-plane query failed closed (${response.status}) for ${path}`);
  }
  return response.json();
}

async function main() {
  const config = JSON.parse(readFileSync(resolve(root, 'release.config.json'), 'utf8'));
  const token = process.env.RELEASE_CONTROL_TOKEN;
  const repositoryName = process.env.GITHUB_REPOSITORY;
  const expectedRef = process.env.GITHUB_REF;
  if (!token) throw new Error('RELEASE_CONTROL_TOKEN is required for external-control preflight.');
  if (!repositoryName || repositoryName !== config.repository) {
    throw new Error('External-control preflight is restricted to the canonical repository.');
  }
  if (process.env.GITHUB_EVENT_NAME !== 'workflow_dispatch') {
    throw new Error('Release control may run only through an intentional workflow dispatch.');
  }
  const repository = await githubJson(`/repos/${repositoryName}`, token);
  if (expectedRef !== `refs/heads/${repository.default_branch}`) {
    throw new Error('Release workflow must execute from the protected default-branch context.');
  }
  const branch = await githubJson(
    `/repos/${repositoryName}/branches/${encodeURIComponent(repository.default_branch)}`,
    token,
  );
  const environmentNames = ['release-trust', 'release-signing', 'release-publication'];
  const environments = Object.fromEntries(
    await Promise.all(
      environmentNames.map(async (name) => [
        name,
        await githubJson(`/repos/${repositoryName}/environments/${name}`, token),
      ]),
    ),
  );
  const [tagRulesetSummaries, runners, ...environmentMetadata] = await Promise.all([
    githubJson(`/repos/${repositoryName}/rulesets?targets=tag&per_page=100`, token),
    githubJson(`/repos/${repositoryName}/actions/runners?per_page=100`, token),
    ...environmentNames.flatMap((name) => [
      githubJson(`/repos/${repositoryName}/environments/${name}/secrets?per_page=100`, token),
      githubJson(`/repos/${repositoryName}/environments/${name}/variables?per_page=100`, token),
    ]),
  ]);
  const tagRulesets = await Promise.all(
    tagRulesetSummaries.map((ruleset) =>
      githubJson(`/repos/${repositoryName}/rulesets/${String(ruleset.id)}`, token),
    ),
  );
  const environmentSecrets = {};
  const environmentVariables = {};
  for (const [index, name] of environmentNames.entries()) {
    environmentSecrets[name] = environmentMetadata[index * 2].secrets.map((value) => value.name);
    environmentVariables[name] = environmentMetadata[index * 2 + 1].variables.map(
      (value) => value.name,
    );
  }
  validateExternalControls(
    {
      repository: { ...repository, defaultBranchProtected: branch.protected === true },
      environments,
      tagRulesets,
      runnerLabels: (runners.runners ?? []).flatMap((runner) =>
        (runner.labels ?? []).map((label) => label.name),
      ),
      environmentSecrets,
      environmentVariables,
    },
    {
      repository: config.repository,
      defaultBranch: repository.default_branch,
      environments: environmentNames,
      runnerLabels: ['windows-11-arm', 'macos-15-intel', 'macos-15'],
      environmentSecrets: {
        'release-trust': ['RELEASE_TAG_SIGNING_PUBLIC_KEY'],
        'release-signing': [
          'WINDOWS_CSC_LINK',
          'WINDOWS_CSC_KEY_PASSWORD',
          'WINDOWS_SIGNING_THUMBPRINT',
          'MACOS_CSC_LINK',
          'MACOS_CSC_KEY_PASSWORD',
          'APPLE_ID',
          'APPLE_APP_SPECIFIC_PASSWORD',
          'APPLE_TEAM_ID',
        ],
        'release-publication': ['RELEASE_TAG_SIGNING_PUBLIC_KEY'],
      },
      environmentVariables: {
        'release-trust': ['RELEASE_TAG_SIGNER_FINGERPRINTS', 'RELEASE_POLICY_TREE_SHA256'],
        'release-publication': [
          'RELEASE_TAG_SIGNER_FINGERPRINTS',
          'RELEASE_POLICY_TREE_SHA256',
          'RELEASE_EXTERNAL_BLOCKERS_APPROVAL',
          'WINDOWS_SIGNING_THUMBPRINT',
          'APPLE_TEAM_ID',
        ],
      },
    },
  );
  console.log(
    `External release controls verified for ${repositoryName}@${repository.default_branch}: protected default branch and ${environmentNames.length} reviewer-protected environments.`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href)
  await main();
