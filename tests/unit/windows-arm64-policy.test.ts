import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('Windows native architecture policy', () => {
  it('keeps x64 and ARM64 enabled across Rust, packaging, and updates', () => {
    const builder = readFileSync('build/electron-builder.yml', 'utf8');
    const helperBuild = readFileSync('scripts/build-helper.mjs', 'utf8');
    const prepackage = readFileSync('scripts/prepackage-check.mjs', 'utf8');
    const metadata = readFileSync('scripts/release-package-metadata.mjs', 'utf8');
    const owner = readFileSync('helper/keyboard-owner/src/lib.rs', 'utf8');
    const gateway = readFileSync('helper/src/lib.rs', 'utf8');
    const rootPackage = JSON.parse(readFileSync('package.json', 'utf8')) as {
      scripts: Record<string, string>;
    };

    const windows = builder.slice(builder.indexOf('\nwin:'), builder.indexOf('\nmac:'));
    expect(windows).toContain('- x64');
    expect(windows).toContain('- arm64');
    expect(helperBuild).not.toContain('Windows x64 native helpers only');
    expect(prepackage).not.toContain("target === 'win' && arch !== 'x64'");
    expect(metadata).not.toContain("platform === 'win' && architecture !== 'x64'");
    expect(owner).not.toContain('Windows ARM64 native owners are forbidden');
    expect(gateway).not.toContain('Windows ARM64 native helpers are forbidden');
    expect(rootPackage.scripts['package:unsigned:win:arm64']).toContain('win-arm64-unsigned');
  });
});
