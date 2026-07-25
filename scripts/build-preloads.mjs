import { resolve } from 'node:path';
import { build } from 'vite';

const appRoot = resolve(import.meta.dirname, '..', 'app');
const roles = ['main', 'widget', 'capture'];

for (const [index, role] of roles.entries()) {
  await build({
    configFile: false,
    root: appRoot,
    logLevel: 'warn',
    build: {
      target: 'node22',
      outDir: resolve(appRoot, 'out', 'preload'),
      emptyOutDir: index === 0,
      sourcemap: false,
      minify: false,
      lib: {
        entry: resolve(appRoot, 'src', 'preload', `${role}.ts`),
        formats: ['cjs'],
        fileName: () => `${role}.js`,
      },
      rollupOptions: {
        external: ['electron'],
        output: {
          inlineDynamicImports: true,
        },
      },
    },
  });
}

console.log('Built isolated, standalone preload bundles for main, widget, and capture');
