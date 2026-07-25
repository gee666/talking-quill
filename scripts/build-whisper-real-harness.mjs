import { resolve } from 'node:path';
import { build } from 'vite';

const root = resolve(import.meta.dirname, '..');
await build({
  configFile: false,
  root,
  logLevel: 'warn',
  ssr: { noExternal: ['zod'] },
  build: {
    target: 'node22',
    outDir: resolve(root, 'tmp', 'tests'),
    emptyOutDir: false,
    sourcemap: false,
    minify: false,
    ssr: resolve(root, 'tests', 'real', 'whisper-real-host.ts'),
    rollupOptions: {
      external: ['electron'],
      output: {
        format: 'cjs',
        entryFileNames: 'whisper-real-host.cjs',
        inlineDynamicImports: true,
      },
    },
  },
});
console.log('Built ModelManager-based real Whisper harness');
