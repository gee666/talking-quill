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
  if (production) {
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
        externalizeDeps: { exclude: ['zod', 'write-file-atomic'] },
        rollupOptions: {
          input: {
            index: resolve(__dirname, 'src/main/index.ts'),
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
