import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const root = resolve(import.meta.dirname, '..');
const config = JSON.parse(readFileSync(resolve(root, 'release.config.json'), 'utf8'));
const promotionGateScripts = ['scripts/verify-draft-release.mjs'];
const fullReleaseTree =
  existsSync(resolve(root, 'package.json')) ||
  existsSync(resolve(root, '.github/workflows/release-unsigned.yml'));
if (fullReleaseTree) {
  for (const path of promotionGateScripts) {
    if (!existsSync(resolve(root, path))) {
      throw new Error(`Required release trust path is missing: ${path}`);
    }
  }
}
// Audit the complete candidate working tree, including newly added source
// files before they are committed. Untracked agent plans are local user input,
// but a plan added to the index is a release-integrity failure.
const indexed = git(['ls-files', '--cached']).split(/\r?\n/u).filter(Boolean);
const indexedPlans = indexed.filter((path) => path.startsWith('.agent-plans/'));
if (indexedPlans.length > 0) {
  throw new Error(`Tracked agent plans are forbidden: ${indexedPlans.join(', ')}`);
}
const untracked = git(['ls-files', '--others', '--exclude-standard'])
  .split(/\r?\n/u)
  .filter((path) => path.length > 0 && !path.startsWith('.agent-plans/'));
const tracked = [...new Set([...indexed, ...untracked])];
if (
  existsSync(resolve(root, 'reference')) ||
  tracked.some((path) => path.startsWith('reference/'))
) {
  throw new Error('The implementation reference tree must not exist in a release candidate.');
}

const approvedTopLevel = config.approvedTopLevel.toSorted();
const actualTopLevel = [...new Set(tracked.map((path) => path.split('/')[0]))].sort();
if (JSON.stringify(actualTopLevel) !== JSON.stringify(approvedTopLevel)) {
  throw new Error(
    `Top-level release tree differs from release.config.json: ${actualTopLevel.join(', ')}`,
  );
}
const forbiddenRoots = [
  'browser-extension/',
  'collector/',
  'docker/',
  'frontend/',
  'server/',
  'reference/',
];
const leftovers = tracked.filter((path) =>
  forbiddenRoots.some((rootPath) => path.startsWith(rootPath)),
);
if (leftovers.length > 0) throw new Error(`Legacy source remains: ${leftovers.join(', ')}`);

const textExtensions = new Set([
  '',
  '.cjs',
  '.css',
  '.html',
  '.js',
  '.json',
  '.jsx',
  '.lock',
  '.md',
  '.mjs',
  '.npmrc',
  '.plist',
  '.ps1',
  '.rs',
  '.toml',
  '.ts',
  '.tsx',
  '.txt',
  '.yaml',
  '.yml',
]);
const rules = new Map([
  ['legacy-brand', /anything(?:[\s-])*llm/iu],
  [
    'legacy-commercial-phrases',
    /\b(?:prisma|vectordb|pro tier|license keys?|usage (?:limits?|counters?)|billing entitlement)\b/iu,
  ],
]);
const exceptionSets = new Map(
  Object.entries(config.textAuditExceptions).map(([rule, paths]) => [rule, new Set(paths)]),
);
const violations = [];
let textualFiles = 0;
for (const path of tracked) {
  if (!textExtensions.has(extname(path).toLowerCase())) continue;
  const absolutePath = resolve(root, path);
  // A release implementation can delete a tracked source before its commit is
  // created; audit the candidate working tree rather than reopening tombstones.
  if (!existsSync(absolutePath)) continue;
  const bytes = readFileSync(absolutePath);
  if (bytes.includes(0)) continue;
  textualFiles += 1;
  const source = bytes.toString('utf8');
  for (const [rule, pattern] of rules) {
    if (pattern.test(source) && !exceptionSets.get(rule)?.has(path))
      violations.push(`${rule}:${path}`);
  }
}
if (violations.length > 0)
  throw new Error(`Forbidden tracked text requires review: ${violations.join(', ')}`);
for (const [rule, paths] of exceptionSets) {
  if (!rules.has(rule)) throw new Error(`Unknown text-audit exception rule: ${rule}`);
  for (const path of paths)
    if (!tracked.includes(path)) throw new Error(`Stale text-audit exception: ${rule}:${path}`);
}
console.log(
  `Release audit passed: approved ${actualTopLevel.length}-entry top level, ${textualFiles} tracked text files audited, no unreviewed legacy terms.`,
);

function git(args) {
  return execFileSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    env: sanitizedSubprocessEnvironment(),
  });
}
