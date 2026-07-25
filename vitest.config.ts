import { resolve } from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@shared': resolve(__dirname, 'app/src/shared'),
      '@renderer': resolve(__dirname, 'app/src/renderer'),
    },
  },
  test: {
    include: ['tests/**/*.test.{ts,tsx}'],
    environment: 'node',
    // Native Electron/SQLite suites are memory-heavy; cap forks for deterministic cross-OS CI.
    maxWorkers: 4,
    setupFiles: ['tests/setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json-summary'],
      include: ['app/src/**/*.{ts,tsx}'],
      exclude: ['app/src/**/main.tsx', 'app/src/main/index.ts', 'app/src/preload/*.ts'],
      thresholds: {
        lines: 72,
        statements: 70,
        functions: 68,
        branches: 65,
      },
    },
  },
});
