import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import { delimiter, resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { PiProvider } from '../../app/src/main/providers/pi';
import { startMockProviderServer, type MockProviderServer } from '../helpers/mock-provider-server';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const required = process.env.TALKING_QUILL_REQUIRE_PI_FIXTURE === '1';
const suite = required ? describe : describe.skip;

suite('owned npm-installed Pi against a nonbillable localhost provider', () => {
  let root = '';
  let agent = '';
  let wrapper = '';
  let argsLog = '';
  let extensionMarker = '';
  let server: MockProviderServer;
  const prefix = process.env.TALKING_QUILL_REAL_NPM_PI_PREFIX ?? '';

  beforeAll(async () => {
    if (prefix.length === 0) throw new Error('TALKING_QUILL_REAL_NPM_PI_PREFIX is required');
    root = await createTestDirectory('pi-owned-real');
    agent = resolve(root, 'agent');
    argsLog = resolve(root, 'args.log');
    extensionMarker = resolve(root, 'extension-loaded.txt');
    await mkdir(resolve(agent, 'extensions'), { recursive: true });
    server = await startMockProviderServer((request, response) => {
      if (JSON.stringify(request.body).includes('HANG_UNTIL_CANCELLED')) return;
      response.writeHead(200, {
        'content-type': 'text/event-stream',
        connection: 'close',
        'cache-control': 'no-cache',
      });
      response.write(
        `data: ${JSON.stringify({
          id: 'local-evidence',
          object: 'chat.completion.chunk',
          created: 1,
          model: 'mock-model',
          choices: [
            {
              index: 0,
              delta: { role: 'assistant', content: 'LOCAL_NONBILLABLE_OK' },
              finish_reason: null,
            },
          ],
        })}\n\n`,
      );
      response.write(
        `data: ${JSON.stringify({
          id: 'local-evidence',
          object: 'chat.completion.chunk',
          created: 1,
          model: 'mock-model',
          choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
        })}\n\n`,
      );
      response.end('data: [DONE]\n\n');
    });
    await writeFile(
      resolve(agent, 'models.json'),
      JSON.stringify({
        providers: {
          'talking-quill-local': {
            baseUrl: `${server.origin}/v1`,
            api: 'openai-completions',
            apiKey: 'synthetic-local-only-key',
            models: [{ id: 'mock-model', contextWindow: 8192, maxTokens: 128 }],
          },
        },
      }),
    );
    await writeFile(resolve(agent, 'APPEND_SYSTEM.md'), 'GLOBAL_APPEND_SYSTEM_EVIDENCE');
    await writeFile(
      resolve(agent, 'extensions', 'evidence.ts'),
      `import { writeFileSync } from 'node:fs';\nwriteFileSync(${JSON.stringify(extensionMarker)}, 'loaded');\nexport default function evidence() {}\n`,
    );
    const installed =
      process.platform === 'win32' ? resolve(prefix, 'pi.cmd') : resolve(prefix, 'bin/pi');
    if (process.platform === 'win32') {
      wrapper = resolve(root, 'Pi Wrapper.CmD');
      await writeFile(wrapper, `@echo off\r\n>>"${argsLog}" echo %*\r\ncall "${installed}" %*\r\n`);
    } else {
      wrapper = resolve(root, 'pi-wrapper');
      await writeFile(
        wrapper,
        `#!/bin/sh\nprintf '%s\\n' "$*" >> '${argsLog}'\nexec '${installed}' "$@"\n`,
      );
      await chmod(wrapper, 0o755);
    }
  }, 30_000);

  afterAll(async () => {
    await server.close();
    if (root) await removeTestDirectory(root);
  });

  it('proves list parity, fixed Test Connection, exact argv/stdin, extensions, and disabled tools', async () => {
    const environment = {
      ...process.env,
      PATH: `${prefix}${delimiter}${process.env.PATH ?? ''}`,
      PI_CODING_AGENT_DIR: agent,
      NO_COLOR: '1',
    };
    const provider = new PiProvider({
      environment,
      platform: process.platform,
      configuredPath: () => wrapper,
      workingDirectory: root,
    });
    const invocation = {
      config: {
        providerId: 'pi' as const,
        modelId: 'talking-quill-local/mock-model',
        thinking: 'high' as const,
      },
      credential: null,
    };
    const models = await provider.listModels(
      { ...invocation, refreshModels: true },
      AbortSignal.timeout(30_000),
    );
    expect(models.map(({ id }) => id)).toContain('talking-quill-local/mock-model');
    await expect(provider.validate(invocation, AbortSignal.timeout(30_000))).resolves.toMatchObject(
      { ok: true },
    );
    await expect(
      provider.cleanTranscript(
        invocation,
        { input: 'EXACT_STDIN_PROMPT' },
        AbortSignal.timeout(30_000),
      ),
    ).resolves.toBe('LOCAL_NONBILLABLE_OK');
    const logged = await readFile(argsLog, 'utf8');
    expect(logged).toContain('--list-models');
    expect(logged).toContain(
      '-p --model talking-quill-local/mock-model --thinking high --no-tools --no-session --no-context-files --no-approve --no-skills --no-prompt-templates --no-themes --offline',
    );
    expect(await readFile(extensionMarker, 'utf8')).toBe('loaded');
    const completionRequests = server.requests.filter(({ url }) =>
      url.endsWith('/chat/completions'),
    );
    expect(completionRequests).toHaveLength(2);
    expect(JSON.stringify(completionRequests[0]?.body)).toContain('TALKING_QUILL_CONNECTION_OK');
    expect(JSON.stringify(completionRequests[1]?.body)).toContain('EXACT_STDIN_PROMPT');
    for (const request of completionRequests) {
      expect(request.body).not.toHaveProperty('tools');
      expect(JSON.stringify(request.body)).toContain('GLOBAL_APPEND_SYSTEM_EVIDENCE');
    }
  }, 90_000);

  it('cancels a real Pi command blocked on the localhost provider', async () => {
    const provider = new PiProvider({
      environment: {
        ...process.env,
        PATH: `${prefix}${delimiter}${process.env.PATH ?? ''}`,
        PI_CODING_AGENT_DIR: agent,
        NO_COLOR: '1',
      },
      platform: process.platform,
      configuredPath: () => wrapper,
      workingDirectory: root,
    });
    const controller = new AbortController();
    const completion = provider.cleanTranscript(
      {
        config: {
          providerId: 'pi',
          modelId: 'talking-quill-local/mock-model',
          thinking: 'high',
        },
        credential: null,
      },
      { input: 'HANG_UNTIL_CANCELLED' },
      controller.signal,
    );
    const deadline = Date.now() + 20_000;
    while (
      !server.requests.some(({ body }) => JSON.stringify(body).includes('HANG_UNTIL_CANCELLED')) &&
      Date.now() < deadline
    )
      await new Promise<void>((resolveWait) => setTimeout(resolveWait, 25));
    controller.abort();
    await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
  }, 60_000);
});
