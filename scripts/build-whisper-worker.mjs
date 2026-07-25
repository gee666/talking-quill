import { readFile } from 'node:fs/promises';
import { isBuiltin } from 'node:module';
import { resolve } from 'node:path';
import { build } from 'vite';

const appRoot = resolve(import.meta.dirname, '..', 'app');
const outputDirectory = resolve(appRoot, 'out', 'workers');

await build({
  configFile: false,
  root: appRoot,
  logLevel: 'warn',
  resolve: {
    alias: {
      sharp: resolve(appRoot, 'src', 'workers', 'whisper', 'sharp-unavailable.ts'),
    },
    conditions: ['node', 'import'],
  },
  ssr: {
    noExternal: ['@huggingface/transformers', 'zod'],
  },
  build: {
    target: 'node22',
    outDir: outputDirectory,
    emptyOutDir: true,
    sourcemap: false,
    minify: false,
    ssr: resolve(appRoot, 'src', 'workers', 'whisper', 'index.ts'),
    rollupOptions: {
      external: ['onnxruntime-node', 'onnxruntime-common'],
      output: {
        format: 'cjs',
        entryFileNames: 'whisper-payload.cjs',
        inlineDynamicImports: true,
      },
    },
  },
});

await build({
  configFile: false,
  root: appRoot,
  logLevel: 'warn',
  build: {
    target: 'node22',
    outDir: outputDirectory,
    emptyOutDir: false,
    sourcemap: false,
    minify: false,
    ssr: resolve(appRoot, 'src', 'workers', 'whisper', 'bootstrap.ts'),
    rollupOptions: {
      output: {
        format: 'cjs',
        entryFileNames: 'whisper-bootstrap.cjs',
        inlineDynamicImports: true,
      },
    },
  },
});

const bootstrap = await readFile(resolve(outputDirectory, 'whisper-bootstrap.cjs'), 'utf8');
const payload = await readFile(resolve(outputDirectory, 'whisper-payload.cjs'), 'utf8');
for (const forbidden of [
  '@huggingface/transformers',
  'onnxruntime-node',
  'onnxruntime-common',
  'zod',
]) {
  if (bootstrap.includes(forbidden)) {
    throw new Error(`Minimal Whisper bootstrap unexpectedly contains ${forbidden}`);
  }
}
const guardInstallation = bootstrap.indexOf('installWorkerNetworkGuard();');
const payloadLoad = bootstrap.indexOf('("./whisper-payload.cjs")');
if (guardInstallation < 0 || payloadLoad <= guardInstallation) {
  throw new Error('Whisper bootstrap does not load the payload after guard installation.');
}
const bootstrapSpecifiers = [
  ...bootstrap.matchAll(/\brequire\("([^"]+)"\)/g),
  ...bootstrap.matchAll(/\bimport\("([^"]+)"\)/g),
].map((match) => match[1]);
if (bootstrapSpecifiers.some((specifier) => specifier === undefined || !isBuiltin(specifier))) {
  throw new Error('Whisper bootstrap contains a non-built-in module dependency.');
}
if (!payload.includes('onnxruntime-node')) {
  throw new Error('Whisper payload does not contain the external ONNX runtime import.');
}
console.log('Built guarded Whisper bootstrap and standalone offline production payload');
