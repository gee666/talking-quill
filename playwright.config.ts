import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 45_000,
  globalTimeout: 30 * 60_000,
  expect: { timeout: 8_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  outputDir: 'tmp/playwright-results',
  projects: [
    {
      name: 'electron',
      testMatch: /source(?:-(?:audio|task6|task7|task10|task12|vocabulary))?\.spec\.ts/,
    },
    {
      name: 'packaged',
      testMatch: /packaged\.spec\.ts/,
    },
  ],
});
