import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'electron-vite';

const rendererRoot = resolve(__dirname, 'src/renderer');
const harnessEnvironment = [
  'TALKING_QUILL_TASK6_TEST_HARNESS',
  'TALKING_QUILL_VOCABULARY_TEST_HARNESS',
  'TALKING_QUILL_PI_TEST_HARNESS',
] as const;

export default defineConfig(({ mode }) => {
  const production = mode === 'production';
  const packageVariant = process.env.TALKING_QUILL_PACKAGE_VARIANT ?? 'canonical';
  if (!['canonical', 'installed-acceptance', 'packaged-test'].includes(packageVariant)) {
    throw new Error(`Unknown package variant: ${packageVariant}`);
  }
  if (!production && packageVariant === 'installed-acceptance') {
    throw new Error('Installed acceptance requires a production-mode build');
  }
  if (production && packageVariant === 'packaged-test') {
    throw new Error('Packaged-test entry requires test build mode');
  }
  const acceptanceBuild = production && packageVariant === 'installed-acceptance';
  const packagedTestBuild = packageVariant === 'packaged-test';
  const acceptanceManifestPublicKey =
    process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL ?? '';
  if (acceptanceBuild && !/^[A-Za-z0-9_-]+$/u.test(acceptanceManifestPublicKey)) {
    throw new Error('Acceptance builds require a pinned P-256 manifest public key');
  }
  if (!acceptanceBuild && acceptanceManifestPublicKey.length > 0) {
    throw new Error('Acceptance authorization material requires the installed-acceptance variant');
  }
  const mainEntry = acceptanceBuild
    ? 'src/main/entries/windows-installed-acceptance.ts'
    : packagedTestBuild
      ? 'src/main/entries/packaged-test.ts'
      : production
        ? 'src/main/index.ts'
        : 'src/main/entries/development.ts';
  if (production && !packagedTestBuild) {
    const poisoned = harnessEnvironment.filter((name) => process.env[name] === '1');
    if (poisoned.length > 0) {
      throw new Error(`Production build rejects test harnesses: ${poisoned.join(', ')}`);
    }
  }
  const sourceRevision = execFileSync('git', ['rev-parse', '--short=12', 'HEAD'], {
    encoding: 'utf8',
  }).trim();
  return {
    main: {
      define: {
        __TALKING_QUILL_SOURCE_REVISION__: JSON.stringify(sourceRevision),
        __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__: JSON.stringify(
          acceptanceManifestPublicKey,
        ),
        __TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD__: JSON.stringify(
          process.env.TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD === '1',
        ),
        __TALKING_QUILL_TASK6_TEST_HARNESS__: JSON.stringify(
          !production && process.env.TALKING_QUILL_TASK6_TEST_HARNESS === '1',
        ),
        __TALKING_QUILL_VOCABULARY_TEST_HARNESS__: JSON.stringify(
          !production && process.env.TALKING_QUILL_VOCABULARY_TEST_HARNESS === '1',
        ),
        __TALKING_QUILL_PI_TEST_HARNESS__: JSON.stringify(
          !production && process.env.TALKING_QUILL_PI_TEST_HARNESS === '1',
        ),
      },
      build: {
        externalizeDeps: { exclude: ['electron-updater', 'zod', 'write-file-atomic'] },
        rollupOptions: {
          input: {
            index: resolve(__dirname, mainEntry),
          },
        },
      },
    },
    preload: {
      build: {
        externalizeDeps: { exclude: ['zod'] },
        // The dev wrapper stages isolated widget/capture preloads before electron-vite builds main.
        ...(production ? {} : { emptyOutDir: false }),
        rollupOptions: { input: resolve(__dirname, 'src/preload/main.ts') },
      },
    },
    renderer: {
      root: rendererRoot,
      // React Fast Refresh injects an inline preamble that strict renderer CSP correctly blocks.
      // Vite's external dev client still provides CSP-compatible full-page updates without it.
      plugins: production ? [react()] : [],
      resolve: {
        alias: {
          '@shared': resolve(__dirname, 'src/shared'),
          '@renderer': rendererRoot,
        },
      },
      build: {
        assetsInlineLimit: 0,
        rollupOptions: {
          input: {
            main: resolve(rendererRoot, 'main/index.html'),
            widget: resolve(rendererRoot, 'widget/index.html'),
            capture: resolve(rendererRoot, 'capture/index.html'),
          },
        },
      },
    },
  };
});
