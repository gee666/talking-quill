import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const sections = {
  validate: workflow.slice(workflow.indexOf('\n  validate:'), workflow.indexOf('\n  package:')),
  package: workflow.slice(workflow.indexOf('\n  package:'), workflow.indexOf('\n  assemble:')),
};

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
});
