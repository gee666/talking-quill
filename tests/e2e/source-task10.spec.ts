import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { _electron as electron, expect, test, type ElectronApplication } from '@playwright/test';
import { rendererPages, resetProfile, widgetPage } from './helpers';
import { DETERMINISTIC_JPEG_BASE64 } from './support/task6-test-composition';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
if (typeof electronModule !== 'string') throw new Error('Electron executable is unavailable');

type ServerMode = 'success' | 'failure' | 'empty' | 'fence' | 'oversized' | 'pending';
interface DriverSnapshot {
  readonly session: { readonly phase: string; readonly abortReason: string | null };
  readonly recording: { readonly starts: number };
  readonly insertion: { readonly calls: number; readonly targetText: string };
  readonly history: readonly {
    readonly outcome: string;
    readonly rawText: string | null;
    readonly processedText: string | null;
    readonly providerId: string | null;
    readonly modelId: string | null;
    readonly hasScreenshot?: boolean;
  }[];
}

function driver<Result = void>(
  application: ElectronApplication,
  method: string,
  args: readonly unknown[] = [],
): Promise<Result> {
  return application.evaluate(
    (_electron, input) => {
      const value: unknown = Reflect.get(globalThis, Symbol.for('talking-quill:task6-test-driver'));
      if (typeof value !== 'object' || value === null) throw new Error('Driver unavailable');
      const operation: unknown = (value as Readonly<Record<string, unknown>>)[input.method];
      if (typeof operation !== 'function') throw new Error('Driver operation unavailable');
      return (operation as (...parameters: readonly unknown[]) => unknown)(...input.args);
    },
    { method, args },
  ) as Promise<Result>;
}

async function snapshot(application: ElectronApplication): Promise<DriverSnapshot> {
  return driver<DriverSnapshot>(application, 'snapshot');
}

function imageUrls(body: unknown): string[] {
  const urls: string[] = [];
  const visit = (value: unknown): void => {
    if (Array.isArray(value)) {
      value.forEach(visit);
      return;
    }
    if (value === null || typeof value !== 'object') return;
    const record = value as Readonly<Record<string, unknown>>;
    if (
      record.type === 'image_url' &&
      typeof record.image_url === 'object' &&
      record.image_url !== null
    ) {
      const url = (record.image_url as Readonly<Record<string, unknown>>).url;
      if (typeof url === 'string') urls.push(url);
    }
    Object.values(record).forEach(visit);
  };
  visit(body);
  return urls;
}

async function quickSubmit(application: ElectronApplication, captureNumber: number): Promise<void> {
  await driver(application, 'activationComplete', [100]);
  await expect.poll(async () => (await snapshot(application)).recording.starts).toBe(captureNumber);
  await expect.poll(async () => (await snapshot(application)).session.phase).toBe('recordingQuick');
  await driver(application, 'frames', [0.2, 15]);
  await driver(application, 'key', ['enter']);
}

test('Task 10 Smart success, OSA one-image request, fallback, and cancellation are real Electron flows', async () => {
  test.setTimeout(90_000);
  let mode: ServerMode = 'success';
  const requestBodies: unknown[] = [];
  let pendingConnectionClosed = false;
  const pendingTimers = new Set<NodeJS.Timeout>();
  const server = createServer((request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
      response.statusCode = 404;
      response.end();
      return;
    }
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk: string) => {
      body += chunk;
    });
    request.on('end', () => {
      requestBodies.push(JSON.parse(body) as unknown);
      if (mode === 'pending') {
        response.once('close', () => {
          pendingConnectionClosed = true;
        });
        const timer = setTimeout(() => {
          pendingTimers.delete(timer);
          if (!response.writableEnded) {
            response.statusCode = 503;
            response.end(JSON.stringify({ error: { message: 'cancelled test cleanup' } }));
          }
        }, 5_000);
        timer.unref();
        pendingTimers.add(timer);
        return;
      }
      response.setHeader('content-type', 'application/json');
      if (mode === 'failure') {
        response.statusCode = 503;
        response.end(JSON.stringify({ error: { message: 'unavailable' } }));
        return;
      }
      const content =
        mode === 'empty'
          ? '   '
          : mode === 'fence'
            ? '```text\n\n```'
            : mode === 'oversized'
              ? 'x'.repeat(1_000_001)
              : '```text\nPolished result\n```';
      response.end(JSON.stringify({ choices: [{ message: { content } }] }));
    });
  });
  await new Promise<void>((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  if (typeof address !== 'object' || address === null) throw new Error('Mock server unavailable');

  const profile = await resetProfile(
    `task10-smart-osa-${String(process.pid)}-${Date.now().toString(36)}`,
  );
  const application = await electron.launch({
    executablePath: electronModule,
    args: [resolve('app'), `--talking-quill-user-data=${profile}`, '--talking-quill-task6-test'],
    env: { ...process.env, NODE_ENV: 'test' },
  });
  try {
    const { main } = await rendererPages(application);
    await expect
      .poll(() =>
        application.evaluate(() =>
          Reflect.has(globalThis, Symbol.for('talking-quill:task6-test-driver')),
        ),
      )
      .toBe(true);
    await main
      .evaluate(
        async (baseUrl) => {
          let stage = 'save-config';
          try {
            const saved = await window.talkingQuill.providers.saveConfig({
              providerId: 'generic-openai',
              baseUrl,
              modelId: 'gpt-4.1',
            });
            stage = 'set-secret';
            await window.talkingQuill.providers.setSecret(
              'generic-openai',
              saved.credentialState.bindingToken,
              'task10-local-key',
            );
            stage = 'update-profile';
            await window.talkingQuill.profiles.update('general', {
              processingMode: 'smart',
            });
            stage = 'profile-ready';
          } catch (error) {
            throw new Error(
              `Task 10 setup failed at ${stage}: ${error instanceof Error ? error.message : String(error)}`,
            );
          }
        },
        `http://127.0.0.1:${String(address.port)}/v1`,
      )
      .catch((error: unknown) => {
        throw new Error(
          `Task 10 setup failed after ${String(requestBodies.length)} provider request(s)`,
          { cause: error },
        );
      });
    await driver(application, 'setOnScreenAwareness', [true]);
    await expect
      .poll(() => main.evaluate(() => window.talkingQuill.providers.osaStatus()))
      .toMatchObject({
        providerId: 'generic-openai',
        modelId: 'gpt-4.1',
        capability: 'supported',
      });
    mode = 'success';
    requestBodies.length = 0;

    await quickSubmit(application, 1);
    await expect.poll(async () => (await snapshot(application)).session.phase).toBe('completed');
    expect(requestBodies).toHaveLength(1);
    expect(imageUrls(requestBodies[0])).toEqual([
      `data:image/jpeg;base64,${DETERMINISTIC_JPEG_BASE64}`,
    ]);
    expect((await snapshot(application)).insertion.targetText).toBe('Polished result');
    expect((await snapshot(application)).history.at(0)).toMatchObject({
      outcome: 'smart-completed',
      rawText: 'deterministic transcript',
      processedText: 'Polished result',
      providerId: 'generic-openai',
      modelId: 'gpt-4.1',
    });

    await expect.poll(async () => (await snapshot(application)).session.phase).toBe('idle');
    mode = 'failure';
    await quickSubmit(application, 2);
    const widget = await widgetPage(application);
    await expect.poll(async () => (await snapshot(application)).session.phase).toBe('completed');
    await expect(widget.getByText('Done', { exact: true })).toBeVisible();
    await expect(
      widget.getByText('The raw transcript was used. Check the provider in Settings.', {
        exact: true,
      }),
    ).toBeVisible();
    expect((await snapshot(application)).insertion.targetText).toBe('deterministic transcript');
    expect((await snapshot(application)).history.at(0)).toMatchObject({
      outcome: 'smart-fallback',
      rawText: 'deterministic transcript',
    });

    let captureNumber = 3;
    for (const invalidMode of ['empty', 'fence', 'oversized'] as const) {
      await expect.poll(async () => (await snapshot(application)).session.phase).toBe('idle');
      mode = invalidMode;
      await quickSubmit(application, captureNumber);
      captureNumber += 1;
      await expect.poll(async () => (await snapshot(application)).session.phase).toBe('completed');
      expect((await snapshot(application)).history.at(0)).toMatchObject({
        outcome: 'smart-fallback',
        rawText: 'deterministic transcript',
      });
    }

    await expect.poll(async () => (await snapshot(application)).session.phase).toBe('idle');
    mode = 'pending';
    pendingConnectionClosed = false;
    const beforeCancel = await snapshot(application);
    const requestsBeforeCancel = requestBodies.length;
    await quickSubmit(application, captureNumber);
    await expect
      .poll(async () => (await snapshot(application)).session.phase)
      .toBe('processingSmart');
    await expect.poll(() => requestBodies.length).toBe(requestsBeforeCancel + 1);
    await driver(application, 'key', ['escape']);
    await expect.poll(async () => (await snapshot(application)).session.phase).toBe('cancelled');
    await expect.poll(() => pendingConnectionClosed).toBe(true);
    const cancelled = await snapshot(application);
    expect(cancelled.insertion.calls).toBe(beforeCancel.insertion.calls);
    expect(cancelled.history).toHaveLength(beforeCancel.history.length);
  } finally {
    for (const timer of pendingTimers) clearTimeout(timer);
    pendingTimers.clear();
    await application.close().catch(() => {
      application.process().kill();
    });
    server.closeAllConnections();
    await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
  }
});
