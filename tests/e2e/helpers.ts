import { mkdir, rm, writeFile } from 'node:fs/promises';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { resolve } from 'node:path';
import type { ElectronApplication, Page } from '@playwright/test';

export async function resetProfile(name: string): Promise<string> {
  const profile = await resetFreshProfile(name);
  const settings = structuredClone(DEFAULT_SETTINGS);
  settings.welcome = {
    completedAt: 1,
    lastStep: 5,
    microphoneTested: true,
    activationTested: true,
  };
  await writeFile(resolve(profile, 'settings.json'), `${JSON.stringify(settings)}\n`, 'utf8');
  return profile;
}

export async function resetFreshProfile(name: string): Promise<string> {
  const profile = resolve('tmp', 'e2e', name);
  await rm(profile, { recursive: true, force: true, maxRetries: 3 });
  await mkdir(profile, { recursive: true });
  return profile;
}

export async function rendererPages(application: ElectronApplication) {
  await application.firstWindow();
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const pages = application.windows();
    const main = pages.find((page) => page.url().includes('/main/index.html'));
    const widget = pages.find((page) => page.url().includes('/widget/index.html'));
    const capture = pages.find((page) => page.url().includes('/capture/index.html'));
    if (main !== undefined && widget !== undefined && capture !== undefined) {
      return { main, widget, capture } as const;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(
    `Expected all window roles; received ${application
      .windows()
      .map((page) => page.url())
      .join(', ')}`,
  );
}

export async function rendererIsolation(page: Page) {
  return page.evaluate(() => ({
    requireType: typeof Reflect.get(globalThis, 'require'),
    processType: typeof Reflect.get(globalThis, 'process'),
    bufferType: typeof Reflect.get(globalThis, 'Buffer'),
    moduleType: typeof Reflect.get(globalThis, 'module'),
    localStorage: Object.fromEntries(
      Array.from({ length: localStorage.length }, (_value, index) => {
        const key = localStorage.key(index) ?? '';
        return [key, localStorage.getItem(key)];
      }),
    ),
  }));
}
