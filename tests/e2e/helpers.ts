import { mkdir, rm, writeFile } from 'node:fs/promises';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { resolve } from 'node:path';
import type { ElectronApplication, Page } from '@playwright/test';

const APPLICATION_SHUTDOWN_DIAGNOSTIC_TIMEOUT_MS = 20_000;

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

export async function closeSourceApplication(
  application: ElectronApplication,
  label: string,
): Promise<void> {
  let closedNormally = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const readyDeadline = Date.now() + 10_000;
    while (
      !(await application.evaluate(
        () => typeof Reflect.get(globalThis, '__talkingQuillRequestQuit') === 'function',
      ))
    ) {
      if (Date.now() >= readyDeadline) {
        throw new Error('Source application did not finish startup');
      }
      await new Promise((resolveWait) => setTimeout(resolveWait, 25));
    }
    const closed = application.waitForEvent('close', { timeout: 0 });
    await application.evaluate(() => {
      const requestQuit = Reflect.get(globalThis, '__talkingQuillRequestQuit') as () => void;
      requestQuit();
    });
    await Promise.race([
      closed,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${label} exceeded the production shutdown deadline`)),
          APPLICATION_SHUTDOWN_DIAGNOSTIC_TIMEOUT_MS,
        );
      }),
    ]);
    closedNormally = true;
  } catch (error: unknown) {
    const diagnostics = await Promise.race([
      application.evaluate(({ BrowserWindow }) => ({
        progress: Reflect.get(globalThis, '__talkingQuillShutdownProgress') as unknown,
        windows: BrowserWindow.getAllWindows().map((window) => ({
          title: window.getTitle(),
          destroyed: window.isDestroyed(),
          webContentsDestroyed: window.webContents.isDestroyed(),
        })),
      })),
      new Promise<{ readonly progress: string; readonly windows: readonly [] }>((resolveWait) => {
        setTimeout(
          () => resolveWait({ progress: 'main process unresponsive', windows: [] }),
          1_000,
        );
      }),
    ]).catch(() => ({ progress: 'main process unavailable', windows: [] }));
    throw new Error(`${String(error)}; shutdown diagnostics: ${JSON.stringify(diagnostics)}`);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    if (!closedNormally) await forceCloseSourceApplication(application);
  }
}

async function forceCloseSourceApplication(application: ElectronApplication): Promise<void> {
  const closing = application.close().catch(() => undefined);
  await Promise.race([closing, new Promise((resolveWait) => setTimeout(resolveWait, 1_000))]);
  const child = application.process();
  if (child.exitCode === null && child.signalCode === null) child.kill();
  await Promise.race([closing, new Promise((resolveWait) => setTimeout(resolveWait, 1_000))]);
}

export async function rendererPages(application: ElectronApplication) {
  // Do not wait on firstWindow(): all persistent windows may already exist before Playwright
  // subscribes. Renderer readiness also keeps test-driver calls behind application initialization.
  const deadline = Date.now() + 10_000;
  let consecutiveReadyObservations = 0;
  while (Date.now() < deadline) {
    const pages = application.windows();
    const main = pages.find((page) => page.url().includes('/main/index.html'));
    const capture = pages.find((page) => page.url().includes('/capture/index.html'));
    const widget = pages.find((page) => page.url().includes('/widget/index.html'));
    if (main !== undefined && capture !== undefined && widget !== undefined) {
      const [captureReady, widgetReady] = await Promise.all([
        capture
          .evaluate(() => document.documentElement.dataset.ready === 'true')
          .catch(() => false),
        widget
          .evaluate(() => document.querySelector('#root')?.hasChildNodes() === true)
          .catch(() => false),
      ]);
      if (captureReady && widgetReady) {
        consecutiveReadyObservations += 1;
        if (consecutiveReadyObservations >= 2) return { main, capture } as const;
      } else consecutiveReadyObservations = 0;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(
    `Expected persistent main, capture, and widget window roles; received ${application
      .windows()
      .map((page) => page.url())
      .join(', ')}`,
  );
}

/** Returns the persistent widget renderer used by the production session lifecycle. */
export async function widgetPage(application: ElectronApplication): Promise<Page> {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const widget = application
      .windows()
      .find((page) => !page.isClosed() && page.url().includes('/widget/index.html'));
    if (widget !== undefined) return widget;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(
    `Expected an active-session widget; received ${application
      .windows()
      .map((page) => page.url())
      .join(', ')}`,
  );
}

export async function widgetIsVisible(application: ElectronApplication): Promise<boolean> {
  return application.evaluate(({ BrowserWindow }) => {
    const widget = BrowserWindow.getAllWindows().find(
      (window) => window.getTitle() === 'Talking Quill Widget',
    );
    return widget?.isVisible() ?? false;
  });
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
