import { mkdir, readFile, readdir, symlink, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { createAppPaths, validateAppRootBeforeUse } from '../../app/src/main/persistence/paths';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';
import {
  RendererLoader,
  selectDevelopmentRendererUrl,
} from '../../app/src/main/app/renderer-loader';
import {
  CAPTURE_PRODUCTION_CSP,
  PRODUCTION_CSP,
  developmentCsp,
} from '../../app/src/main/security/csp';

describe('security and paths foundations', () => {
  it('keeps every owned path under injected userData', () => {
    const root = resolve('tmp', 'tests', 'injected-root');
    const paths = createAppPaths(root);
    const ownedPaths = [
      paths.root,
      paths.settingsFile,
      paths.historyDatabase,
      paths.credentialsFile,
      paths.screenshots,
      paths.models,
      paths.modelTemporary,
      paths.logs,
      paths.temporary,
    ];
    for (const ownedPath of ownedPaths) expect(ownedPath.startsWith(root)).toBe(true);
    expect(paths.modelTemporary).toContain('models');
  });

  it('rejects a userData junction before any child write and leaves its target byte-identical', async () => {
    const temporary = await createTestDirectory('linked-user-data-root');
    try {
      const base = resolve(temporary, 'base');
      const external = resolve(temporary, 'external');
      const root = resolve(base, 'Talking Quill');
      await mkdir(base);
      await mkdir(external);
      await writeFile(resolve(external, 'sentinel.bin'), Buffer.from([0, 1, 2, 255]));
      await symlink(external, root, process.platform === 'win32' ? 'junction' : 'dir');
      const before = await readFile(resolve(external, 'sentinel.bin'));
      expect(() => validateAppRootBeforeUse(createAppPaths(root), base, false)).toThrow(
        /symbolic-link|junction/u,
      );
      expect(await readdir(external)).toEqual(['sentinel.bin']);
      expect(await readFile(resolve(external, 'sentinel.bin'))).toEqual(before);
    } finally {
      await removeTestDirectory(temporary);
    }
  });

  it('gates development renderers and DevTools from trusted application mode', () => {
    expect(selectDevelopmentRendererUrl(true, 'http://localhost:5173')).toBeUndefined();
    expect(selectDevelopmentRendererUrl(false, 'http://localhost:5173')).toBe(
      'http://localhost:5173',
    );
    expect(new RendererLoader(undefined).allowsDevTools).toBe(false);
    expect(new RendererLoader('http://localhost:5173').allowsDevTools).toBe(true);
  });

  it('allows workers only in capture CSP and explicitly denies them for UI roles', () => {
    expect(PRODUCTION_CSP).toContain("default-src 'none'");
    expect(PRODUCTION_CSP).toContain("connect-src 'none'");
    expect(PRODUCTION_CSP).toContain("worker-src 'none'");
    expect(PRODUCTION_CSP).toContain("img-src 'self' blob:");
    expect(PRODUCTION_CSP).not.toContain('data:');
    expect(CAPTURE_PRODUCTION_CSP).toContain("worker-src 'self'");
    expect(developmentCsp('http://localhost:5173')).toContain("worker-src 'none'");
    expect(developmentCsp('http://localhost:5173', true)).toContain("worker-src 'self'");
    expect(PRODUCTION_CSP).not.toContain('unsafe-inline');
    expect(PRODUCTION_CSP).not.toContain('unsafe-eval');
  });

  it('packages a scoped macOS microphone usage description', async () => {
    const builder = await readFile(resolve('build/electron-builder.yml'), 'utf8');
    expect(builder).toContain('NSMicrophoneUsageDescription:');
    expect(builder).toContain('only while you record dictation or test your input level');
  });

  it('defines the exact approved colors, AA pairings, and reduced-motion fallback', async () => {
    const tokens = await readFile(resolve('app/src/renderer/design/tokens.css'), 'utf8');
    const global = await readFile(resolve('app/src/renderer/design/global.css'), 'utf8');
    for (const color of [
      // Light theme.
      '#F7F8FA',
      '#FFFFFF',
      '#FBFCFD',
      '#E4E8EE',
      '#16202E',
      '#5A6879',
      '#34618F',
      '#1F7A55',
      '#8A5C0B',
      '#B23A37',
      // Dark theme.
      '#161B23',
      '#1B212B',
      '#1F2530',
      '#2C3441',
      '#DDE4EE',
      '#98A4B5',
      '#7AA8D8',
      '#5CC9A0',
      '#E0B063',
      '#E88B84',
    ]) {
      expect(tokens.toUpperCase()).toContain(color);
    }
    const contrastPairs: readonly (readonly [string, string])[] = [
      // Light theme: ink and supporting copy on every surface.
      ['#16202e', '#f7f8fa'],
      ['#16202e', '#ffffff'],
      ['#16202e', '#fbfcfd'],
      ['#5a6879', '#f7f8fa'],
      ['#5a6879', '#ffffff'],
      ['#5a6879', '#fbfcfd'],
      // Light theme: accent and status tones on their own tinted backgrounds.
      ['#ffffff', '#34618f'],
      ['#34618f', '#e8eff7'],
      ['#1f7a55', '#e2f2eb'],
      ['#8a5c0b', '#f8eeda'],
      ['#b23a37', '#fbe9e8'],
      // Dark theme: ink and supporting copy on every surface.
      ['#dde4ee', '#161b23'],
      ['#dde4ee', '#1b212b'],
      ['#dde4ee', '#1f2530'],
      ['#98a4b5', '#161b23'],
      ['#98a4b5', '#1b212b'],
      ['#98a4b5', '#1f2530'],
      // Dark theme: accent, brand and status tones on their own tinted backgrounds.
      ['#0e1622', '#7aa8d8'],
      ['#7aa8d8', '#222f40'],
      ['#dcbb80', '#1b212b'],
      ['#5cc9a0', '#18302a'],
      ['#e0b063', '#2d2519'],
      ['#e88b84', '#2e1a1c'],
    ];
    for (const [foreground, background] of contrastPairs) {
      expect(contrastRatio(foreground, background)).toBeGreaterThanOrEqual(4.5);
    }
    expect(tokens).toContain("[data-theme='light']");
    expect(tokens).toContain("[data-theme='dark']");
    expect(global).toContain('prefers-reduced-motion: reduce');
  });

  it('keeps spacing tokens and literal layout spacing on the approved 4px scale', async () => {
    const tokens = await readFile(resolve('app/src/renderer/design/tokens.css'), 'utf8');
    for (const [name, pixels] of [
      ['--space-1', 4],
      ['--space-2', 8],
      ['--space-3', 12],
      ['--space-4', 16],
      ['--space-5', 24],
      ['--space-6', 32],
    ] as const) {
      expect(tokens).toMatch(new RegExp(`${name}:\\s*${String(pixels)}px;`));
    }

    const files = [
      'app/src/renderer/design/components.css',
      'app/src/renderer/design/global.css',
      'app/src/renderer/main/main.css',
      'app/src/renderer/main/styles/screens.css',
      'app/src/renderer/main/styles/settings.css',
      'app/src/renderer/main/styles/welcome.css',
      'app/src/renderer/widget/widget.css',
    ];
    const declaration =
      /^\s*(?:gap|row-gap|column-gap|padding(?:-[\w-]+)?|margin(?:-[\w-]+)?):\s*([^;]+);/gm;
    for (const file of files) {
      const css = await readFile(resolve(file), 'utf8');
      for (const match of css.matchAll(declaration)) {
        for (const value of match[1]?.matchAll(/(-?\d+(?:\.\d+)?)px/g) ?? []) {
          const pixels = Number(value[1]);
          // 1px is the reserved hairline gap that separates grouped rows.
          if (pixels === -1 || pixels === 1) continue;
          expect(pixels % 2, `${file}: ${match[0]}`).toBe(0);
        }
      }
    }
  });
});

function contrastRatio(first: string, second: string): number {
  const firstLuminance = relativeLuminance(first);
  const secondLuminance = relativeLuminance(second);
  const lighter = Math.max(firstLuminance, secondLuminance);
  const darker = Math.min(firstLuminance, secondLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}

function relativeLuminance(hex: string): number {
  const channels = [hex.slice(1, 3), hex.slice(3, 5), hex.slice(5, 7)].map((channel) => {
    const value = Number.parseInt(channel, 16) / 255;
    return value <= 0.039_28 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return (channels[0] ?? 0) * 0.2126 + (channels[1] ?? 0) * 0.7152 + (channels[2] ?? 0) * 0.0722;
}
