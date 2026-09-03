import { generateKeyPairSync, verify } from 'node:crypto';
import net from 'node:net';
import { describe, expect, it, vi } from 'vitest';
import {
  canonicalAcceptanceJson,
  createOneUseJsonChannel,
  createSignedAcceptanceRequest,
  runPackagedAcceptanceProbe,
  spawnPackagedProcess,
} from '../../scripts/windows-installed-acceptance-probe.mjs';

const runWindow = (notBeforeMs: number) => ({
  notBeforeMs,
  expiresAtMs: notBeforeMs + 80 * 60_000,
  maxTotalRunMs: 80 * 60_000,
});

const invocation = (
  invocationId: string,
  latestStartOffsetMs: number,
  deadlineOffsetMs: number,
) => ({ invocationId, latestStartOffsetMs, deadlineOffsetMs });

describe('Windows installed acceptance packaged probe transport', () => {
  it('creates a canonical P-256 request bound to the exact command and build', () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const encoded = createSignedAcceptanceRequest('heartbeat-120s', {
      buildId: '11'.repeat(32),
      privateKeyPem: keys.privateKey.export({ format: 'pem', type: 'pkcs8' }).toString(),
      nowMs: 1_000_000,
      timeoutMs: 150_000,
      ...invocation('heartbeat-120s', 18 * 60_000, 21 * 60_000),
      runWindow: runWindow(1_000_000),
      expiresAtMs: 1_000_000 + 21 * 60_000,
      readinessPipe: '\\\\.\\pipe\\TalkingQuill.InstalledReadiness.' + '22'.repeat(16),
      armedPipe: '\\\\.\\pipe\\TalkingQuill.AutomationArmed.' + '22'.repeat(16),
      launchCorrelation: '33'.repeat(32),
    });
    const envelope = JSON.parse(Buffer.from(encoded, 'base64url').toString('utf8')) as {
      payload: Record<string, unknown>;
      signatureBase64url: string;
    };
    expect(envelope.payload).toMatchObject({
      command: 'heartbeat-120s',
      buildId: '11'.repeat(32),
      heartbeatDurationMs: 120_000,
      physicalObservation: false,
      automationValidation: false,
    });
    expect(
      verify(
        'sha256',
        Buffer.from(canonicalAcceptanceJson(envelope.payload)),
        { key: keys.publicKey, dsaEncoding: 'ieee-p1363' },
        Buffer.from(envelope.signatureBase64url, 'base64url'),
      ),
    ).toBe(true);
  });

  it('accepts exactly one bounded newline-framed response', async () => {
    const socket = `\\\\.\\pipe\\TalkingQuill.InstalledReadiness.${String(Date.now()).padStart(32, '0')}`;
    const channel = createOneUseJsonChannel(socket, 2_000);
    await channel.listening;
    await new Promise<void>((resolveWrite, reject) => {
      const client = net.connect(socket);
      client.once('error', reject);
      client.once('connect', () => client.end('{"version":1,"result":"passed"}\n', resolveWrite));
    });
    await expect(channel.value).resolves.toEqual({ version: 1, result: 'passed' });
    channel.close();
  });

  it('confirms a cancelled packaged process has exited before teardown resolves', async () => {
    const child = spawnPackagedProcess(
      process.execPath,
      ['-e', 'setInterval(() => {}, 1000)'],
      30_000,
    );
    await child.terminateIfRunning();
    await expect(child.exited).rejects.toThrow('exited');
  });

  it('does not expose the acceptance request private key to a spawned artifact', async () => {
    process.env.TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY_PEM = 'must-not-cross';
    try {
      const child = spawnPackagedProcess(
        process.execPath,
        [
          '-e',
          'process.exit(process.env.TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY_PEM === undefined ? 0 : 91)',
        ],
        5_000,
      );
      await expect(child.exited).resolves.toBe(0);
    } finally {
      delete process.env.TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY_PEM;
    }
  });

  it('actively cancels a physical probe and awaits process teardown', async () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const signedRequest = createSignedAcceptanceRequest('manual-physical-observation', {
      buildId: '11'.repeat(32),
      privateKeyPem: keys.privateKey.export({ format: 'pem', type: 'pkcs8' }).toString(),
      nowMs: Date.now(),
      timeoutMs: 30_000,
      ...invocation('manual-physical-observation', 57 * 60_000, 59 * 60_000),
      runWindow: runWindow(Date.now() - 1_000),
      expiresAtMs: Date.now() + 59 * 60_000,
      readinessPipe: '\\\\.\\pipe\\TalkingQuill.InstalledReadiness.' + '77'.repeat(16),
      armedPipe: '\\\\.\\pipe\\TalkingQuill.AutomationArmed.' + '77'.repeat(16),
      launchCorrelation: '88'.repeat(32),
    });
    const terminateIfRunning = vi.fn(async () => {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
    });
    const controller = new AbortController();
    const operation = runPackagedAcceptanceProbe('manual-physical-observation', {
      executable: 'Talking Quill.exe',
      buildId: '11'.repeat(32),
      signedRequest,
      timeoutMs: 30_000,
      signal: controller.signal,
      spawnProcess: () => ({
        pid: 42,
        exited: new Promise<number>(() => undefined),
        terminateIfRunning,
      }),
    });
    setTimeout(() => controller.abort(), 10);
    await expect(operation).rejects.toThrow('actively cancelled');
    expect(terminateIfRunning).toHaveBeenCalled();
  });

  it('launches the exact executable only after both one-use channels are listening', async () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const spawnProcess = vi.fn(
      (
        executable: string,
        commandArguments: readonly string[],
        _timeoutMs: number,
        startupFrame: Buffer,
      ) => {
        expect(executable).toBe('C:/Program Files/Talking Quill/Talking Quill.exe');
        expect(commandArguments).toEqual(['--talking-quill-installed-acceptance-fd=3']);
        const frame = JSON.parse(startupFrame.toString('utf8')) as { arguments: string[] };
        const value = (prefix: string) =>
          frame.arguments.find((argument) => argument.startsWith(prefix))?.slice(prefix.length);
        const correlation = value('--talking-quill-launch-correlation=');
        const pipe = value('--talking-quill-installed-readiness-pipe=');
        if (pipe === undefined || correlation === undefined)
          throw new Error('missing probe arguments');
        const client = net.connect(pipe);
        client.once('connect', () =>
          client.end(
            `${JSON.stringify({ version: 1, result: 'passed', correlation, runtimeLifecycleAuthoritative: false, userDataRootSha256: '66'.repeat(32) })}\n`,
          ),
        );
        return {
          pid: 42,
          exited: Promise.resolve(0),
          terminateIfRunning: vi.fn(() => Promise.resolve()),
        };
      },
    );
    const signedRequest = createSignedAcceptanceRequest('normal-readiness', {
      buildId: '11'.repeat(32),
      privateKeyPem: keys.privateKey.export({ format: 'pem', type: 'pkcs8' }).toString(),
      nowMs: Date.now(),
      timeoutMs: 2_000,
      ...invocation('profile-normal-readiness', 15 * 60_000, 16 * 60_000),
      runWindow: runWindow(Date.now() - 1_000),
      expiresAtMs: Date.now() + 16 * 60_000,
      readinessPipe: '\\\\.\\pipe\\TalkingQuill.InstalledReadiness.' + '44'.repeat(16),
      armedPipe: '\\\\.\\pipe\\TalkingQuill.AutomationArmed.' + '44'.repeat(16),
      launchCorrelation: '55'.repeat(32),
    });
    await expect(
      runPackagedAcceptanceProbe('normal-readiness', {
        executable: 'C:/Program Files/Talking Quill/Talking Quill.exe',
        buildId: '11'.repeat(32),
        signedRequest,
        timeoutMs: 2_000,
        spawnProcess,
      }),
    ).resolves.toMatchObject({ result: 'passed' });
    expect(spawnProcess).toHaveBeenCalledOnce();
    expect(spawnProcess.mock.calls[0]?.[0]).toBe(
      'C:/Program Files/Talking Quill/Talking Quill.exe',
    );
  });
});
