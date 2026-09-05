import type { SpawnOptionsWithoutStdio } from 'node:child_process';
import { describe, expect, it, vi } from 'vitest';
import type { PiCliIdentity } from '../../app/src/main/providers/pi-executable';
import {
  PI_RPC_PROTOCOL_VERSION,
  PI_RPC_REQUIRED_SAFETY_FLAGS,
  PiRpcTimingStage,
  assertRpcCompatibility,
  createPiRpcArguments,
  prewarmPiRpcOperation,
  type PiRpcPrewarmOptions,
} from '../../app/src/main/providers/pi-rpc-operation';
import {
  DEFAULT_PI_RPC_LIMITS,
  PiRpcNdjsonDecoder,
} from '../../app/src/main/providers/pi-rpc-transport';
import { ScriptedPiRpcFixture } from '../fixtures/scripted-pi-rpc';

const expected = Object.freeze({
  provider: 'fixture-provider',
  model: 'fixture-model',
  thinking: 'high' as const,
});

function identity(canonicalPath = '/opt/pi'): PiCliIdentity {
  return Object.freeze({
    canonicalPath,
    packageVersion: PI_RPC_PROTOCOL_VERSION,
    safetyFlags: PI_RPC_REQUIRED_SAFETY_FLAGS,
    fileIdentity: { dev: '1', ino: '2', size: 3, mtimeMs: 4 },
  });
}

function readiness(id: unknown = 'talking-quill-state-1') {
  return {
    id,
    type: 'response',
    command: 'get_state',
    success: true,
    data: {
      model: { provider: expected.provider, id: expected.model },
      thinkingLevel: expected.thinking,
      isStreaming: false,
      isCompacting: false,
      steeringMode: 'all',
      followUpMode: 'one-at-a-time',
      sessionId: 'ephemeral-session',
      autoCompactionEnabled: true,
      messageCount: 0,
      pendingMessageCount: 0,
    },
  } as const;
}

function assistantMessage(
  stopReason: 'pending' | 'stop' | 'error' = 'stop',
  content: readonly Readonly<Record<string, unknown>>[] = [
    { type: 'thinking', thinking: 'not output' },
    { type: 'text', text: 'cleaned' },
    { type: 'text', text: ' text' },
  ],
) {
  return {
    role: 'assistant',
    api: 'fixture-api',
    provider: expected.provider,
    model: expected.model,
    content,
    usage: {},
    stopReason,
    timestamp: 1,
  } as const;
}

function realShapeAssistantMessage(): Readonly<Record<string, unknown>> {
  return {
    role: 'assistant',
    content: [
      {
        type: 'text',
        text: 'real-shape output',
        textSignature: 'synthetic-signature',
      },
    ],
    api: 'fixture-api',
    provider: expected.provider,
    model: expected.model,
    usage: {
      input: 1,
      output: 1,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 2,
      reasoning: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: 'stop',
    timestamp: 1,
    rawStopReason: 'stop',
    responseId: 'synthetic-response',
  };
}

function reverseJsonObjectKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(reverseJsonObjectKeys);
  if (typeof value !== 'object' || value === null) return value;
  return Object.fromEntries(
    Object.entries(value)
      .reverse()
      .map(([key, item]) => [key, reverseJsonObjectKeys(item)]),
  );
}

function successfulRunRecords(
  fixture: ScriptedPiRpcFixture,
  promptId = 'talking-quill-prompt-1',
  input?: string,
): readonly Readonly<Record<string, unknown>>[] {
  const final = assistantMessage();
  const promptInput =
    input ??
    [...fixture.commands].reverse().find(({ type, id }) => type === 'prompt' && id === promptId)
      ?.message;
  if (typeof promptInput !== 'string') throw new Error('fixture prompt input is missing');
  const user = {
    role: 'user',
    content: [{ type: 'text', text: promptInput }],
    timestamp: 1,
  } as const;
  return [
    { id: promptId, type: 'response', command: 'prompt', success: true },
    { type: 'agent_start' },
    { type: 'turn_start' },
    { type: 'message_start', message: user },
    { type: 'message_end', message: user },
    { type: 'message_start', message: assistantMessage('pending', []) },
    {
      type: 'message_update',
      usage: {},
      assistantMessageEvent: {
        type: 'thinking_delta',
        contentIndex: 0,
        delta: 'ignore\u2028this\u2029thinking',
      },
    },
    {
      type: 'message_update',
      usage: {},
      assistantMessageEvent: {
        type: 'text_delta',
        contentIndex: 1,
        delta: 'misleading delta',
      },
    },
    { type: 'message_end', message: final },
    { type: 'turn_end', message: final, toolResults: [] },
    { type: 'agent_end', messages: [user, final] },
    { type: 'agent_settled' },
  ];
}

function emitSuccessfulRun(
  fixture: ScriptedPiRpcFixture,
  promptId = 'talking-quill-prompt-1',
  input?: string,
): void {
  fixture.sendRecords(successfulRunRecords(fixture, promptId, input));
}

function fixtureOptions(
  fixture: ScriptedPiRpcFixture,
  overrides: Partial<PiRpcPrewarmOptions> = {},
): PiRpcPrewarmOptions {
  return {
    identity: identity(),
    expected,
    platform: 'linux',
    environment: {},
    workingDirectory: '.',
    timeoutMs: 2_000,
    abortGraceMs: 5,
    retirementGraceMs: 5,
    spawnPi: () => fixture.child,
    terminateTree: () => {
      fixture.close(null, 'SIGKILL');
      return Promise.resolve();
    },
    ...overrides,
  };
}

function readinessFixture(options: ConstructorParameters<typeof ScriptedPiRpcFixture>[0] = {}) {
  return new ScriptedPiRpcFixture({
    ...options,
    onCommand: (command, fixture) => {
      if (command.type === 'get_state') fixture.send(readiness(command.id));
      options.onCommand?.(command, fixture);
    },
  });
}

describe('Pi RPC strict NDJSON transport', () => {
  it('parses fragmented/coalesced UTF-8 records using LF only, optional CR, and Unicode separators', () => {
    const decoder = new PiRpcNdjsonDecoder(DEFAULT_PI_RPC_LIMITS);
    const first = Buffer.from('{"type":"one","value":"😀\u2028x\u2029"}\r\n', 'utf8');
    const emoji = first.indexOf(Buffer.from('😀'));
    expect(decoder.push(first.subarray(0, emoji + 1))).toEqual([]);
    const records = decoder.push(
      Buffer.concat([first.subarray(emoji + 1), Buffer.from('{"type":"two"}\n')]),
    );
    expect(records).toEqual([{ type: 'one', value: '😀\u2028x\u2029' }, { type: 'two' }]);
    decoder.finish();
  });

  it.each([
    Buffer.from([0x7b, 0x22, 0x78, 0x22, 0x3a, 0x22, 0xc3, 0x28, 0x22, 0x7d, 0x0a]),
    Buffer.from('{"type":"x","type":"y"}\n'),
    Buffer.from('\n'),
    Buffer.from('\uFEFF{"type":"x"}\n'),
  ])('rejects malformed UTF-8/JSON records', (record) => {
    const decoder = new PiRpcNdjsonDecoder(DEFAULT_PI_RPC_LIMITS);
    expect(() => decoder.push(record)).toThrow(
      expect.objectContaining({ code: 'INVALID_RESPONSE' }),
    );
  });

  it('rejects oversized and unterminated records', () => {
    const decoder = new PiRpcNdjsonDecoder({
      ...DEFAULT_PI_RPC_LIMITS,
      maxRecordBytes: 8,
    });
    expect(() => decoder.push(Buffer.from('123456789'))).toThrow(
      expect.objectContaining({ code: 'RESPONSE_TOO_LARGE' }),
    );
    const truncated = new PiRpcNdjsonDecoder(DEFAULT_PI_RPC_LIMITS);
    truncated.push(Buffer.from('{"type":"x"}'));
    expect(() => truncated.finish()).toThrow(expect.objectContaining({ code: 'INVALID_RESPONSE' }));
  });
});

describe('single-use supported Pi RPC operation', () => {
  it('pins the capability boundary and builds ordered RPC/extension argv', () => {
    expect(
      createPiRpcArguments(identity(), expected, ['/trusted/one.ts', '/trusted/two.ts']),
    ).toEqual([
      '--mode',
      'rpc',
      '--provider',
      expected.provider,
      '--model',
      expected.model,
      '--thinking',
      expected.thinking,
      ...PI_RPC_REQUIRED_SAFETY_FLAGS,
      '-e',
      '/trusted/one.ts',
      '-e',
      '/trusted/two.ts',
    ]);
    expect(() =>
      assertRpcCompatibility({
        packageVersion: '0.84.3',
        safetyFlags: PI_RPC_REQUIRED_SAFETY_FLAGS,
      }),
    ).not.toThrow();
    expect(() =>
      assertRpcCompatibility({
        packageVersion: '0.84.4',
        safetyFlags: PI_RPC_REQUIRED_SAFETY_FLAGS,
      }),
    ).toThrow(expect.objectContaining({ code: 'PI_INCOMPATIBLE' }));
  });

  it('rejects invalid RPC bounds before spawning', async () => {
    const spawnPi = vi.fn();
    await expect(
      prewarmPiRpcOperation({
        identity: identity(),
        expected,
        spawnPi,
        limits: { maxRecordBytes: 0 },
      }),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    expect(spawnPi).not.toHaveBeenCalled();
  });

  it('uses the fixed Windows cmd bridge while keeping the prompt on persistent stdin', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type === 'prompt') emitSuccessfulRun(current, command.id as string);
      },
    });
    const calls: {
      executable: string;
      args: readonly string[];
      options: SpawnOptionsWithoutStdio;
    }[] = [];
    const operation = await prewarmPiRpcOperation(
      fixtureOptions(fixture, {
        identity: identity('C:\\Users\\Example User\\pi.CMD'),
        platform: 'win32',
        environment: {
          SystemRoot: 'C:\\Windows',
          ComSpec: 'C:\\Windows\\System32\\cmd.exe',
        },
        spawnPi: (executable, args, options) => {
          calls.push({ executable, args, options });
          return fixture.child;
        },
      }),
    );
    const result = await operation.prompt('prompt & never shell-expanded');
    expect(result.text).toBe('cleaned text');
    expect(calls).toHaveLength(1);
    expect(calls[0]?.executable).toBe('C:\\Windows\\System32\\cmd.exe');
    expect(calls[0]?.args.slice(0, 3)).toEqual(['/d', '/s', '/c']);
    expect(calls[0]?.args[3]).not.toContain('prompt & never shell-expanded');
    expect(calls[0]?.options).toMatchObject({ shell: false, windowsVerbatimArguments: true });
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
    await result.retirement;
  });

  it('validates empty no-session readiness before exposing the operation', async () => {
    const fixture = new ScriptedPiRpcFixture({
      onCommand: (command, current) => {
        if (command.type === 'get_state') {
          current.send({
            ...readiness(command.id),
            data: { ...readiness(command.id).data, messageCount: 1 },
          });
        }
      },
    });
    await expect(prewarmPiRpcOperation(fixtureOptions(fixture))).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
    });
  });

  it('accepts the actual 0.84.2 agent_end shape and gates completion on agent_settled', async () => {
    const stages: PiRpcTimingStage[] = [];
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type === 'prompt') emitSuccessfulRun(current, command.id as string);
      },
    });
    const terminateTree = vi.fn(() => {
      fixture.close(null, 'SIGKILL');
      return Promise.resolve();
    });
    const operation = await prewarmPiRpcOperation(
      fixtureOptions(fixture, { onTiming: (stage) => stages.push(stage), terminateTree }),
    );
    const result = await operation.prompt('private transcript');
    expect(result.text).toBe('cleaned text');
    expect(fixture.stdin.writableEnded).toBe(false);
    await result.retirement;
    expect(terminateTree).toHaveBeenCalledOnce();
    expect(stages).toEqual([
      PiRpcTimingStage.ProcessSpawned,
      PiRpcTimingStage.ReadinessProbeWritten,
      PiRpcTimingStage.Ready,
      PiRpcTimingStage.PromptWritten,
      PiRpcTimingStage.PromptAccepted,
      PiRpcTimingStage.AssistantMessageEnded,
      PiRpcTimingStage.AgentSettled,
      PiRpcTimingStage.RetirementStarted,
      PiRpcTimingStage.Retired,
    ]);
  });

  it('accepts real 0.84.2 repeated message snapshots regardless of JSON object key order', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        const final = realShapeAssistantMessage();
        const repeated = reverseJsonObjectKeys(final) as Readonly<Record<string, unknown>>;
        current.sendRecords([
          { id: command.id, type: 'response', command: 'prompt', success: true },
          { type: 'agent_start' },
          { type: 'turn_start' },
          { type: 'message_start', message: assistantMessage('pending', []) },
          { type: 'message_end', message: final },
          { type: 'turn_end', message: repeated, toolResults: [] },
          { type: 'agent_end', messages: [repeated], willRetry: false },
          { type: 'agent_settled' },
        ]);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('private transcript');
    expect(result.text).toBe('real-shape output');
    await result.retirement;
  });

  it('still rejects a changed repeated assistant snapshot before agent_settled', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        const final = realShapeAssistantMessage();
        current.sendRecords([
          { id: command.id, type: 'response', command: 'prompt', success: true },
          { type: 'agent_start' },
          { type: 'turn_start' },
          { type: 'message_start', message: assistantMessage('pending', []) },
          { type: 'message_end', message: final },
          { type: 'turn_end', message: { ...final, timestamp: 2 }, toolResults: [] },
          { type: 'agent_end', messages: [final], willRetry: false },
          { type: 'agent_settled' },
        ]);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('private transcript')).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
      fallbackEligible: false,
    });
  });

  it('does not complete after an actual-shape agent_end when agent_settled is absent', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.sendRecords(successfulRunRecords(current, String(command.id)).slice(0, -1));
        current.close(0, null);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('private transcript')).rejects.toMatchObject({
      code: 'PI_LAUNCH_FAILED',
      fallbackEligible: false,
    });
  });

  it('rejects a present non-boolean agent_end willRetry field', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        const records = successfulRunRecords(current, String(command.id)).map((record) =>
          record.type === 'agent_end' ? { ...record, willRetry: 'false' } : record,
        );
        current.sendRecords(records);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('private transcript')).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
      fallbackEligible: false,
    });
  });

  it('treats a valid present agent_end willRetry field as non-authoritative', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        const records = successfulRunRecords(current, String(command.id)).map((record) =>
          record.type === 'agent_end' ? { ...record, willRetry: true } : record,
        );
        current.sendRecords(records);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('private transcript');
    expect(result.text).toBe('cleaned text');
    await result.retirement;
  });

  it('auto-cancels all blocking extension UI requests and ignores fire-and-forget UI', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.send({
          type: 'extension_ui_request',
          id: 'select-id',
          method: 'select',
          title: 'select',
          options: ['one'],
        });
        current.send({
          type: 'extension_ui_request',
          id: 'confirm-id',
          method: 'confirm',
          title: 'confirm',
          message: 'message',
        });
        current.send({
          type: 'extension_ui_request',
          id: 'input-id',
          method: 'input',
          title: 'input',
        });
        current.send({
          type: 'extension_ui_request',
          id: 'editor-id',
          method: 'editor',
          title: 'editor',
        });
        current.send({
          type: 'extension_ui_request',
          id: 'notify-id',
          method: 'notify',
          message: 'ignored',
        });
        emitSuccessfulRun(current, command.id as string);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('prompt');
    await vi.waitFor(() =>
      expect(fixture.commands.filter(({ type }) => type === 'extension_ui_response')).toHaveLength(
        4,
      ),
    );
    expect(
      fixture.commands
        .filter(({ type }) => type === 'extension_ui_response')
        .map(({ id, cancelled }) => ({ id, cancelled })),
    ).toEqual([
      { id: 'select-id', cancelled: true },
      { id: 'confirm-id', cancelled: true },
      { id: 'input-id', cancelled: true },
      { id: 'editor-id', cancelled: true },
    ]);
    await result.retirement;
  });

  it('uses the final successful retry message without writing the prompt twice', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.send({ id: command.id, type: 'response', command: 'prompt', success: true });
        const failed = assistantMessage('error', [{ type: 'text', text: 'failed output' }]);
        current.send({ type: 'agent_start' });
        current.send({ type: 'turn_start' });
        current.send({ type: 'message_start', message: assistantMessage('pending', []) });
        current.send({ type: 'message_end', message: failed });
        current.send({ type: 'turn_end', message: failed, toolResults: [] });
        current.send({ type: 'agent_end', messages: [failed] });
        current.send({
          type: 'auto_retry_start',
          attempt: 1,
          maxAttempts: 3,
          delayMs: 1,
          errorMessage: 'private provider failure',
        });
        const final = assistantMessage('stop', [{ type: 'text', text: 'final output' }]);
        current.send({ type: 'agent_start' });
        current.send({ type: 'turn_start' });
        current.send({ type: 'message_start', message: assistantMessage('pending', []) });
        current.send({ type: 'message_end', message: final });
        current.send({ type: 'auto_retry_end', success: true, attempt: 1 });
        current.send({ type: 'turn_end', message: final, toolResults: [] });
        current.send({ type: 'agent_end', messages: [final] });
        current.send({ type: 'agent_settled' });
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('one prompt');
    expect(result.text).toBe('final output');
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
    await result.retirement;
  });

  it('waits through an agent continuation and returns only the message before settlement', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.send({ id: command.id, type: 'response', command: 'prompt', success: true });
        const first = assistantMessage('stop', [{ type: 'text', text: 'superseded output' }]);
        current.send({ type: 'agent_start' });
        current.send({ type: 'turn_start' });
        current.send({ type: 'message_start', message: assistantMessage('pending', []) });
        current.send({ type: 'message_end', message: first });
        current.send({ type: 'turn_end', message: first, toolResults: [] });
        current.send({ type: 'agent_end', messages: [first] });
        const final = assistantMessage('stop', [{ type: 'text', text: 'continuation output' }]);
        current.send({ type: 'agent_start' });
        current.send({ type: 'turn_start' });
        current.send({ type: 'message_start', message: assistantMessage('pending', []) });
        current.send({ type: 'message_end', message: final });
        current.send({ type: 'turn_end', message: final, toolResults: [] });
        current.send({ type: 'agent_end', messages: [final] });
        current.send({ type: 'agent_settled' });
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('one prompt');
    expect(result.text).toBe('continuation output');
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
    await result.retirement;
  });

  it.each(['before-response', 'streaming', 'before-settled'] as const)(
    'cooperatively aborts and then force-retires during %s',
    async (phase) => {
      const fixture = readinessFixture({
        closeOnStdinEnd: false,
        onCommand: (command, current) => {
          if (command.type !== 'prompt') return;
          if (phase === 'before-response') return;
          current.send({ id: command.id, type: 'response', command: 'prompt', success: true });
          current.send({ type: 'agent_start' });
          current.send({ type: 'turn_start' });
          current.send({ type: 'message_start', message: assistantMessage('pending', []) });
          if (phase === 'streaming') return;
          const final = assistantMessage();
          current.send({ type: 'message_end', message: final });
          current.send({ type: 'turn_end', message: final, toolResults: [] });
          current.send({ type: 'agent_end', messages: [final] });
        },
      });
      const terminateTree = vi.fn(() => {
        fixture.close(null, 'SIGKILL');
        return Promise.resolve();
      });
      const operation = await prewarmPiRpcOperation(fixtureOptions(fixture, { terminateTree }));
      const controller = new AbortController();
      const completion = operation.prompt('cancel me', controller.signal);
      controller.abort();
      await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
      expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
      expect(fixture.commands.filter(({ type }) => type === 'abort')).toHaveLength(1);
      expect(terminateTree).toHaveBeenCalledOnce();
    },
  );

  it('does not spawn for pre-cancellation and cancels while awaiting readiness', async () => {
    const alreadyCancelled = new AbortController();
    alreadyCancelled.abort();
    const spawnPi = vi.fn();
    await expect(
      prewarmPiRpcOperation({
        identity: identity(),
        expected,
        signal: alreadyCancelled.signal,
        spawnPi,
      }),
    ).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(spawnPi).not.toHaveBeenCalled();

    const spawnRaceFixture = new ScriptedPiRpcFixture({ closeOnStdinEnd: false });
    const spawnRaceController = new AbortController();
    await expect(
      prewarmPiRpcOperation(
        fixtureOptions(spawnRaceFixture, {
          signal: spawnRaceController.signal,
          spawnPi: () => {
            spawnRaceController.abort();
            return spawnRaceFixture.child;
          },
        }),
      ),
    ).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(spawnRaceFixture.commands.some(({ type }) => type === 'get_state')).toBe(false);

    const fixture = new ScriptedPiRpcFixture({ closeOnStdinEnd: false });
    const controller = new AbortController();
    const pending = prewarmPiRpcOperation(fixtureOptions(fixture, { signal: controller.signal }));
    await vi.waitFor(() => expect(fixture.commands).toHaveLength(1));
    controller.abort();
    await expect(pending).rejects.toMatchObject({
      code: 'CANCELLED',
      fallbackEligible: true,
    });
    expect(fixture.commands.map(({ type }) => type)).toEqual(['get_state', 'abort']);
  });

  it('rejects duplicate protocol in the settlement stdout chunk without another provider request', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.sendRecords([
          ...successfulRunRecords(current, String(command.id)),
          { type: 'agent_settled' },
        ]);
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('one write')).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
      fallbackEligible: false,
    });
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('rejects a malformed trailing record in the settlement stdout chunk before sealing', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        const valid = successfulRunRecords(current, String(command.id))
          .map((record) => `${JSON.stringify(record)}\n`)
          .join('');
        current.sendBytes(
          Buffer.concat([
            Buffer.from(valid, 'utf8'),
            Buffer.from([0x7b, 0x22, 0x78, 0x22, 0x3a, 0x22, 0xc3, 0x28, 0x22, 0x7d, 0x0a]),
          ]),
        );
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('one write')).rejects.toMatchObject({
      code: 'INVALID_RESPONSE',
      fallbackEligible: false,
    });
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('seals application protocol and drains later shutdown chatter without reinterpreting it', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        emitSuccessfulRun(current, String(command.id));
        current.sendRecords([
          { id: command.id, type: 'response', command: 'prompt', success: true },
          { type: 'message_end', message: assistantMessage() },
          { type: 'agent_settled' },
        ]);
        current.sendBytes(Buffer.from([0xc3, 0x28, 0x0a]));
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const result = await operation.prompt('prompt');
    expect(result.text).toBe('cleaned text');
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
    await expect(result.retirement).resolves.toBeUndefined();
  });

  it('kills bounded retirement on oversized post-seal output without changing the model result', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        emitSuccessfulRun(current, String(command.id));
        current.sendBytes(Buffer.alloc(64 * 1024));
      },
    });
    const terminateTree = vi.fn(() => {
      fixture.close(null, 'SIGKILL');
      return Promise.resolve();
    });
    const operation = await prewarmPiRpcOperation(
      fixtureOptions(fixture, {
        limits: { maxStdoutBytes: 48 * 1024 },
        terminateTree,
      }),
    );
    const result = await operation.prompt('prompt');
    expect(result.text).toBe('cleaned text');
    await expect(result.retirement).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });
    expect(terminateTree).toHaveBeenCalledOnce();
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('bounds a never-resolving injected tree terminator', async () => {
    const fixture = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type === 'prompt') emitSuccessfulRun(current, command.id as string);
      },
    });
    const operation = await prewarmPiRpcOperation(
      fixtureOptions(fixture, {
        treeTerminationTimeoutMs: 20,
        terminateTree: () => new Promise<never>(() => undefined),
      }),
    );
    const result = await operation.prompt('prompt');
    await expect(result.retirement).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED' });
  });

  it('treats a valid settled provider error as a remote failure, not malformed protocol', async () => {
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type !== 'prompt') return;
        current.send({ id: command.id, type: 'response', command: 'prompt', success: true });
        const failed = assistantMessage('error', []);
        current.send({ type: 'agent_start' });
        current.send({ type: 'turn_start' });
        current.send({ type: 'message_start', message: assistantMessage('pending', []) });
        current.send({ type: 'message_end', message: failed });
        current.send({ type: 'turn_end', message: failed, toolResults: [] });
        current.send({ type: 'agent_end', messages: [failed] });
        current.send({ type: 'agent_settled' });
      },
    });
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    await expect(operation.prompt('prompt')).rejects.toMatchObject({ code: 'REMOTE_FAILURE' });
  });

  it.each(['prompt-response', 'agent-settled'] as const)(
    'continues answering cleanup UI after a valid remote failure at %s',
    async (failureAt) => {
      const fixture = readinessFixture({
        onCommand: (command, current) => {
          if (command.type !== 'prompt') return;
          const failed = assistantMessage('error', []);
          const records =
            failureAt === 'prompt-response'
              ? [
                  {
                    id: command.id,
                    type: 'response',
                    command: 'prompt',
                    success: false,
                    error: 'remote failure',
                  },
                ]
              : [
                  { id: command.id, type: 'response', command: 'prompt', success: true },
                  { type: 'agent_start' },
                  { type: 'turn_start' },
                  { type: 'message_start', message: assistantMessage('pending', []) },
                  { type: 'message_end', message: failed },
                  { type: 'turn_end', message: failed, toolResults: [] },
                  { type: 'agent_end', messages: [failed] },
                  { type: 'agent_settled' },
                ];
          current.sendRecords([
            ...records,
            { type: 'extension_ui_request', id: 'cleanup-ui', method: 'input', title: 'ignored' },
          ]);
        },
      });
      const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
      await expect(operation.prompt('prompt')).rejects.toMatchObject({
        code: 'REMOTE_FAILURE',
        fallbackEligible: false,
      });
      await expect(operation.cleanup).resolves.toBeUndefined();
      await expect(operation.retirement).resolves.toBeUndefined();
      expect(fixture.commands).toContainEqual({
        type: 'extension_ui_response',
        id: 'cleanup-ui',
        cancelled: true,
      });
      expect(fixture.commands.filter(({ type }) => type === 'abort')).toHaveLength(1);
    },
  );

  it('honors cancellation from a settlement timing observer before sealing completion', async () => {
    const controller = new AbortController();
    const fixture = readinessFixture({
      onCommand: (command, current) => {
        if (command.type === 'prompt') emitSuccessfulRun(current, command.id as string);
      },
    });
    const operation = await prewarmPiRpcOperation(
      fixtureOptions(fixture, {
        signal: controller.signal,
        onTiming: (stage) => {
          if (stage === PiRpcTimingStage.AgentSettled) controller.abort();
        },
      }),
    );
    await expect(operation.prompt('prompt')).rejects.toMatchObject({ code: 'CANCELLED' });
    await expect(operation.cleanup).resolves.toBeUndefined();
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });

  it('bounds stdout, stderr, outbound records, EOF, and hung retirement', async () => {
    const oversized = new ScriptedPiRpcFixture({ closeOnStdinEnd: false });
    const oversizedStart = prewarmPiRpcOperation(
      fixtureOptions(oversized, { limits: { maxRecordBytes: 32 } }),
    );
    await vi.waitFor(() => expect(oversized.commands).toHaveLength(1));
    oversized.send(readiness());
    await expect(oversizedStart).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });

    const stderr = new ScriptedPiRpcFixture({ closeOnStdinEnd: false });
    const stderrStart = prewarmPiRpcOperation(
      fixtureOptions(stderr, { limits: { maxStderrBytes: 4 } }),
    );
    stderr.stderr.write('12345');
    await expect(stderrStart).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });

    const outbound = readinessFixture({ closeOnStdinEnd: false });
    const outboundOperation = await prewarmPiRpcOperation(
      fixtureOptions(outbound, { limits: { maxOutboundRecordBytes: 64 } }),
    );
    await expect(outboundOperation.prompt('x'.repeat(128))).rejects.toMatchObject({
      code: 'REQUEST_TOO_LARGE',
      fallbackEligible: true,
    });

    const eof = new ScriptedPiRpcFixture({ closeOnStdinEnd: false });
    const eofStart = prewarmPiRpcOperation(fixtureOptions(eof));
    eof.endStdout();
    await expect(eofStart).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED' });

    const hung = readinessFixture({
      closeOnStdinEnd: false,
      onCommand: (command, current) => {
        if (command.type === 'prompt') emitSuccessfulRun(current, command.id as string);
      },
    });
    const terminateTree = vi.fn(() => {
      hung.close(null, 'SIGKILL');
      return Promise.resolve();
    });
    const hungOperation = await prewarmPiRpcOperation(fixtureOptions(hung, { terminateTree }));
    const result = await hungOperation.prompt('prompt');
    await result.retirement;
    expect(terminateTree).toHaveBeenCalledOnce();
  });

  it('never writes a second prompt', async () => {
    const fixture = readinessFixture();
    const operation = await prewarmPiRpcOperation(fixtureOptions(fixture));
    const first = operation.prompt('first');
    await expect(operation.prompt('second')).rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    await operation.abort();
    await expect(first).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(fixture.commands.filter(({ type }) => type === 'prompt')).toHaveLength(1);
  });
});
