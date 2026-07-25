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
    ssr: resolve(root, 'tests', 'integration', 'whisper-client-real-host.ts'),
    rollupOptions: {
      external: ['electron'],
      output: {
        format: 'cjs',
        entryFileNames: 'whisper-client-real-host.cjs',
        inlineDynamicImports: true,
      },
    },
  },
});
console.log('Built real WhisperWorkerClient utility-process harness');
