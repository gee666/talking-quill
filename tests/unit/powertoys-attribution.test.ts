import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('PowerToys Keyboard Manager attribution', () => {
  it('keeps source attribution, the complete MIT grant, and the generated notice together', async () => {
    const [source, attribution, notices, generator] = await Promise.all([
      readFile('helper/keyboard-owner/src/platform/windows/injection/replay.rs', 'utf8'),
      readFile('docs/attribution/powertoys-mit.txt', 'utf8'),
      readFile('app/assets/THIRD_PARTY_NOTICES.txt', 'utf8'),
      readFile('scripts/generate-notices.mjs', 'utf8'),
    ]);

    expect(source).toContain('fn neutralize_menu(');
    expect(source).toContain('adapted from Microsoft PowerToys Keyboard');
    expect(source).toContain('docs/attribution/powertoys-mit.txt');
    expect(attribution).toContain('Copyright (c) Microsoft Corporation');
    expect(attribution).toContain('Permission is hereby granted, free of charge');
    expect(attribution).toContain('THE SOFTWARE IS PROVIDED "AS IS"');
    expect(generator).toContain('docs/attribution/powertoys-mit.txt');
    expect(notices).toContain('Microsoft PowerToys Keyboard Manager MIT attribution');
    expect(notices).toContain(attribution.trim());
  });
});
