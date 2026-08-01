import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const releaseWorkflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const workflowSources = [
  releaseWorkflow,
  readFileSync('.github/workflows/build-mac-test.yml', 'utf8'),
];
const sections = {
  validate: releaseWorkflow.slice(
    releaseWorkflow.indexOf('\n  validate:'),
    releaseWorkflow.indexOf('\n  package:'),
  ),
  package: releaseWorkflow.slice(
    releaseWorkflow.indexOf('\n  package:'),
    releaseWorkflow.indexOf('\n  assemble:'),
  ),
};
const approvedNode24Actions = new Set([
  'actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09',
  'actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444',
  'actions/upload-artifact@b7c566a772e6b6bfb58ed0dc250532a479d7789f',
  'actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c',
  'pnpm/action-setup@fc06bc1257f339d1d5d8b3a19a8cae5388b55320',
]);
const javascriptActionNames = new Set(
  [...approvedNode24Actions].map((action) => action.slice(0, action.indexOf('@'))),
);

describe('unsigned release workflow', () => {
  it.each([
    ['validate', 'pnpm validate:unsigned-release'],
    ['package', 'pnpm ${{ matrix.command }}'],
  ] as const)(
    'fetches every Rust target graph before the %s job consumes notices',
    (job, consumer) => {
      const fetchIndex = sections[job].indexOf('run: pnpm rust:fetch-targets');
      expect(fetchIndex).toBeGreaterThan(-1);
      expect(fetchIndex).toBeLessThan(sections[job].indexOf(`run: ${consumer}`));
    },
  );

  it('pins JavaScript actions to reviewed Node.js 24 revisions', () => {
    for (const source of workflowSources) {
      const actions = [...source.matchAll(/\buses:\s*([^\s#]+)/gu)].flatMap((match) =>
        match[1] === undefined ? [] : [match[1]],
      );
      expect(actions.length).toBeGreaterThan(0);
      for (const action of actions) {
        expect(action).toMatch(/^[^@\s]+@[a-f0-9]{40}$/u);
        const name = action.slice(0, action.indexOf('@'));
        if (javascriptActionNames.has(name)) {
          expect(approvedNode24Actions).toContain(action);
        }
      }
    }
  });

  it('disables Cargo terminal color before parsing license records', () => {
    const source = readFileSync('scripts/generate-notices.mjs', 'utf8');
    expect(source).toMatch(/'tree',\s*'--color',\s*'never',\s*'--manifest-path'/u);
  });
});
