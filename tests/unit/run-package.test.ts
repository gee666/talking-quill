import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { createPackagePlan } from '../../scripts/run-package.mjs';

const expectedPlans = {
  win: ['package:win', 'nsis', 'win', 'x64'],
  'win-dir': ['package:win:dir', 'none', 'win', 'x64'],
  'win-arm64': ['package:win:arm64', 'nsis', 'win', 'arm64'],
  'win-unsigned': ['package:win:unsigned', 'nsis', 'win', 'x64'],
  'win-arm64-unsigned': ['package:win:arm64:unsigned', 'nsis', 'win', 'arm64'],
  'mac-x64': ['package:mac:x64', 'dmg-zip', 'mac', 'x64'],
  'mac-arm64': ['package:mac:arm64', 'dmg-zip', 'mac', 'arm64'],
  'mac-x64-unsigned': ['package:mac:x64:unsigned', 'dmg-zip', 'mac', 'x64'],
  'mac-arm64-unsigned': ['package:mac:arm64:unsigned', 'dmg-zip', 'mac', 'arm64'],
} as const;

describe('package orchestration', () => {
  it('keeps each target command and security identity in one descriptor', () => {
    for (const [target, [command, artifactRequirement, platform, architecture]] of Object.entries(
      expectedPlans,
    )) {
      expect(createPackagePlan(target)).toEqual({
        command,
        artifactRequirement,
        platform,
        architecture,
        pnpmArguments: ['--filter', '@talking-quill/app', command],
      });
    }
    for (const invalid of ['unknown', '__proto__', 'constructor', 'toString']) {
      expect(() => createPackagePlan(invalid)).toThrow('Expected package target');
    }
  });

  it('delegates prepackage and inspection exactly once to every app package command', async () => {
    const appManifest = JSON.parse(await readFile(resolve('app/package.json'), 'utf8')) as {
      scripts: Record<string, string>;
    };
    const orchestrator = await readFile(resolve('scripts/run-package.mjs'), 'utf8');
    expect(orchestrator).not.toContain('prepackage-check.mjs');
    expect(orchestrator).not.toContain('package:inspect');
    for (const [command] of Object.values(expectedPlans)) {
      expect(appManifest.scripts[command]).toContain('scripts/prepackage-check.mjs');
      expect(appManifest.scripts[command]).toContain('scripts/inspect-package.mjs');
    }
    expect(appManifest.scripts['package:win:arm64:unsigned']).toContain('-c.compression=store');
    expect(appManifest.scripts['package:win:unsigned']).not.toContain('-c.compression=store');
  });
});
