import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';

const root = resolve('.');
const fixture = resolve(root, 'tmp/release-audit-agent-plans');
const auditScript = resolve(root, 'scripts/release-audit.mjs');

beforeEach(() => {
  rmSync(fixture, { recursive: true, force: true });
  mkdirSync(resolve(fixture, 'scripts'), { recursive: true });
  copyFileSync(auditScript, resolve(fixture, 'scripts/release-audit.mjs'));
  writeFileSync(
    resolve(fixture, 'release.config.json'),
    JSON.stringify({
      approvedTopLevel: ['release.config.json', 'scripts'],
      textAuditExceptions: {
        'legacy-brand': ['scripts/release-audit.mjs'],
        'legacy-commercial-phrases': ['scripts/release-audit.mjs'],
      },
    }),
  );
  git(['init', '--quiet']);
  git(['add', 'release.config.json', 'scripts/release-audit.mjs']);
});

describe('release audit agent-plan handling', () => {
  it('ignores and preserves the user-owned untracked plan', () => {
    const plan = resolve(fixture, '.agent-plans/local-plan.md');
    mkdirSync(resolve(fixture, '.agent-plans'), { recursive: true });
    writeFileSync(plan, 'private untracked plan\n');

    expect(() => runAudit()).not.toThrow();
    expect(readFileSync(plan, 'utf8')).toBe('private untracked plan\n');
    expect(git(['status', '--short', '--untracked-files=all'])).toContain(
      '?? .agent-plans/local-plan.md',
    );
  });

  it('requires the promotion trust inventory when auditing a full release tree', () => {
    writeFileSync(resolve(fixture, 'package.json'), '{}\n');
    expect(() => runAudit()).toThrow('Required release trust path is missing');
  });

  it('fails closed when an agent plan is added to the index', () => {
    mkdirSync(resolve(fixture, '.agent-plans'), { recursive: true });
    writeFileSync(resolve(fixture, '.agent-plans/tracked.md'), 'must not ship\n');
    git(['add', '-f', '.agent-plans/tracked.md']);

    expect(() => runAudit()).toThrow('Tracked agent plans are forbidden');
  });
});

function git(args: string[]): string {
  return execFileSync('git', args, { cwd: fixture, encoding: 'utf8' });
}

function runAudit(): string {
  return execFileSync(process.execPath, ['scripts/release-audit.mjs'], {
    cwd: fixture,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}
