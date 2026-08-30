#!/usr/bin/env node
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function authenticateMacosR11Provenance(input) {
  if (
    input.repository !== 'gee666/talking-quill' ||
    input.repositoryData?.full_name !== input.repository
  ) {
    throw new Error('macOS lifecycle provenance repository is not approved');
  }
  if (!/^[0-9a-f]{64}$/u.test(input.baselineZipSha256 ?? '')) {
    throw new Error('Accepted baseline ZIP SHA-256 must be lowercase hexadecimal');
  }
  const defaultBranch = input.repositoryData?.default_branch;
  if (typeof defaultBranch !== 'string' || !defaultBranch)
    throw new Error('Missing default branch');
  const candidate = verifyRun(input.candidateRun, input.repository, defaultBranch, 'candidate');
  const baseline = verifyRun(input.baselineRun, input.repository, defaultBranch, 'baseline');
  if (candidate.runId !== input.candidateRunId || baseline.runId !== input.baselineRunId) {
    throw new Error('GitHub response does not match the requested macOS lifecycle run');
  }
  if (candidate.runId === baseline.runId || candidate.headSha === baseline.headSha) {
    throw new Error('macOS lifecycle predecessor must be a distinct accepted release');
  }
  const candidateArtifact = verifyArtifact(
    input.candidateArtifacts,
    `Talking-Quill-local-owner-mac-${input.arch}`,
    candidate.runId,
  );
  const baselineArtifact = verifyArtifact(
    input.baselineArtifacts,
    `Talking-Quill-local-owner-mac-${input.arch}`,
    baseline.runId,
  );
  return {
    schemaVersion: 1,
    kind: 'macos-r11-authenticated-inputs',
    repository: input.repository,
    workflowPath: '.github/workflows/build-mac-local-owner.yml',
    arch: input.arch,
    policy: { event: 'workflow_dispatch', ref: `refs/heads/${defaultBranch}` },
    candidate: { ...candidate, artifact: candidateArtifact },
    baseline: {
      ...baseline,
      artifact: baselineArtifact,
      acceptedZipSha256: input.baselineZipSha256,
    },
  };
}

export function verifyRun(run, repository, defaultBranch, label) {
  if (
    String(run?.id ?? '') === '' ||
    (run?.head_repository?.full_name ?? run?.repository?.full_name) !== repository ||
    run?.path !== '.github/workflows/build-mac-local-owner.yml' ||
    run?.status !== 'completed' ||
    run?.conclusion !== 'success' ||
    run?.event !== 'workflow_dispatch' ||
    run?.head_branch !== defaultBranch ||
    !/^[0-9a-f]{40}$/u.test(run?.head_sha ?? '') ||
    !Number.isSafeInteger(run?.run_attempt) ||
    run.run_attempt < 1
  ) {
    throw new Error(`Unapproved ${label} macOS workflow provenance`);
  }
  return {
    runId: String(run.id),
    runAttempt: String(run.run_attempt),
    headSha: run.head_sha,
    event: run.event,
    ref: `refs/heads/${run.head_branch}`,
  };
}

export function verifyArtifact(response, name, runId) {
  const matches = (response?.artifacts ?? []).filter(
    (entry) =>
      entry?.name === name &&
      entry?.expired === false &&
      String(entry?.workflow_run?.id ?? '') === runId &&
      Number.isSafeInteger(entry?.id),
  );
  if (matches.length !== 1)
    throw new Error(`macOS workflow artifact is missing or ambiguous: ${name}`);
  return { id: String(matches[0].id), name };
}

function main() {
  const options = parse(process.argv.slice(2));
  const arch = required(options, 'arch');
  if (arch !== 'x64' && arch !== 'arm64') throw new Error('Expected --arch x64|arm64');
  const provenance = authenticateMacosR11Provenance({
    arch,
    repository: required(options, 'repository'),
    repositoryData: json(required(options, 'repository-json')),
    candidateRunId: required(options, 'candidate-run-id'),
    baselineRunId: required(options, 'baseline-run-id'),
    candidateRun: json(required(options, 'candidate-run-json')),
    baselineRun: json(required(options, 'baseline-run-json')),
    candidateArtifacts: json(required(options, 'candidate-artifacts-json')),
    baselineArtifacts: json(required(options, 'baseline-artifacts-json')),
    baselineZipSha256: required(options, 'baseline-zip-sha256'),
  });
  writeFileSync(required(options, 'output'), `${JSON.stringify(provenance, null, 2)}\n`, {
    flag: 'wx',
    mode: 0o600,
  });
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `candidate_run_id=${provenance.candidate.runId}\n`);
    appendFileSync(process.env.GITHUB_OUTPUT, `candidate_commit=${provenance.candidate.headSha}\n`);
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `candidate_run_attempt=${provenance.candidate.runAttempt}\n`,
    );
    appendFileSync(process.env.GITHUB_OUTPUT, `baseline_run_id=${provenance.baseline.runId}\n`);
  }
}

function parse(args) {
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    if (!args[index]?.startsWith('--') || args[index + 1] === undefined)
      throw new Error('Expected --name value');
    const name = args[index].slice(2);
    if (values.has(name)) throw new Error(`Duplicate --${name}`);
    values.set(name, args[index + 1]);
  }
  return values;
}
function required(values, name) {
  const value = values.get(name);
  if (!value) throw new Error(`Missing --${name}`);
  return value;
}
function json(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main();
