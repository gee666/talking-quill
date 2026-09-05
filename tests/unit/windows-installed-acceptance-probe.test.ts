import { generateKeyPairSync, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  canonicalAcceptanceJson,
  createSignedAcceptanceRequest,
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

  it('moves client admission and exact process launch into the native broker', () => {
    const pipe = readFileSync('helper/acceptance-signer/src/broker/pipe_io.rs', 'utf8');
    const identity = readFileSync('helper/acceptance-signer/src/broker/identity.rs', 'utf8');
    const process = readFileSync('helper/acceptance-signer/src/broker/process.rs', 'utf8');
    const adapter = readFileSync('scripts/windows-installed-acceptance-probe.mjs', 'utf8');
    expect(pipe).toContain('named_pipe_client_pid');
    expect(pipe).toContain('DisconnectNamedPipe');
    expect(identity).toContain('peer_identity_matches');
    expect(identity).toContain('creation_chain_reaches_root');
    expect(process).toContain('PROC_THREAD_ATTRIBUTE_HANDLE_LIST');
    expect(pipe).toContain('absolute_deadline_ms');
    expect(adapter).toContain("operation: 'probe'");
    expect(adapter).not.toContain('createOneUseJsonChannel(readinessPipe');
  });

  it('uses a broker-held job for cancellation instead of terminating by PID', () => {
    const process = readFileSync('helper/acceptance-signer/src/broker/process.rs', 'utf8');
    const startup = readFileSync('helper/acceptance-signer/src/broker/startup.rs', 'utf8');
    const adapter = readFileSync('scripts/windows-installed-acceptance-probe.mjs', 'utf8');
    expect(process).toContain('JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE');
    expect(startup).toContain('TerminateJobObject');
    expect(adapter).toContain("events.action('terminate')");
    expect(adapter).not.toContain('taskkill.exe');
  });
});
