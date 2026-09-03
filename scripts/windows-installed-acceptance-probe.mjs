import { createPrivateKey, randomBytes, sign } from 'node:crypto';
import { spawn } from 'node:child_process';
import net from 'node:net';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const MAX_RESPONSE_BYTES = 64 * 1024;
const MAX_ACCEPTANCE_RUN_MS = 80 * 60 * 1_000;

export function canonicalAcceptanceJson(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) throw new Error('Acceptance numbers must be safe integers');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalAcceptanceJson).join(',')}]`;
  if (typeof value !== 'object') throw new Error('Unsupported acceptance value');
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalAcceptanceJson(value[key])}`)
    .join(',')}}`;
}

export function createSignedAcceptanceRequest(command, options) {
  const issuedAtMs = options.nowMs ?? Date.now();
  const physicalObservation = command === 'manual-physical-observation';
  const automationValidation = automationValidationFor(command);
  const payload = {
    version: 1,
    purpose: 'talking-quill/installed-acceptance-run',
    command,
    buildId: options.buildId,
    invocationId: options.invocationId,
    latestStartOffsetMs: options.latestStartOffsetMs,
    deadlineOffsetMs: options.deadlineOffsetMs,
    runWindow: options.runWindow,
    requestNonce: randomBytes(32).toString('hex'),
    issuedAtMs,
    expiresAtMs:
      options.expiresAtMs ??
      issuedAtMs + Math.min(options.timeoutMs + 30_000, MAX_ACCEPTANCE_RUN_MS),
    readinessPipe: options.readinessPipe,
    launchCorrelation: options.launchCorrelation,
    physicalObservation,
    automationValidation,
    automationArmedPipe:
      automationValidation ||
      command === 'login-marker' ||
      command === 'manual-physical-observation'
        ? options.armedPipe
        : null,
    automationCase: automationValidation
      ? command === 'supplemental-synthetic-observation'
        ? 'general'
        : 'lifecycle'
      : null,
    lifecycleUserData: options.lifecycleUserData ?? null,
    heartbeatDurationMs: command === 'heartbeat-120s' ? 120_000 : 6_250,
  };
  const key = createPrivateKey(options.privateKeyPem);
  if (key.asymmetricKeyType !== 'ec' || key.asymmetricKeyDetails?.namedCurve !== 'prime256v1') {
    throw new Error('Acceptance request signing key must be P-256');
  }
  const signatureBase64url = sign('sha256', Buffer.from(canonicalAcceptanceJson(payload)), {
    key,
    dsaEncoding: 'ieee-p1363',
  }).toString('base64url');
  return Buffer.from(canonicalAcceptanceJson({ payload, signatureBase64url })).toString(
    'base64url',
  );
}

function automationValidationFor(command) {
  return (
    command === 'gateway-reconnect-arm' ||
    command === 'electron-crash-arm' ||
    command === 'supplemental-synthetic-observation'
  );
}

export async function runPackagedAcceptanceProbe(command, options) {
  const signedRequest = readFrozenSignedRequest(
    options.signedRequest,
    command,
    options.buildId,
    options.invocation,
    options.nowMs?.() ?? Date.now(),
  );
  const { readinessPipe, launchCorrelation } = signedRequest.payload;
  const armedPipe = signedRequest.payload.automationArmedPipe;
  const resultChannel = createOneUseJsonChannel(readinessPipe, options.timeoutMs);
  const armedChannel =
    armedPipe === null ? null : createOneUseJsonChannel(armedPipe, options.timeoutMs);
  await Promise.all([resultChannel.listening, armedChannel?.listening]);
  const sensitiveArguments = [
    `--talking-quill-acceptance-request=${options.signedRequest}`,
    `--talking-quill-installed-readiness-pipe=${readinessPipe}`,
    `--talking-quill-launch-correlation=${launchCorrelation}`,
  ];
  if (command === 'manual-physical-observation') {
    sensitiveArguments.push('--talking-quill-installed-physical-observation');
  }
  if (armedChannel !== null) {
    sensitiveArguments.push(`--talking-quill-automation-armed-pipe=${armedPipe}`);
    if (automationValidationFor(command)) {
      sensitiveArguments.push(
        '--talking-quill-installed-automation-validation',
        `--talking-quill-automation-case=${
          command === 'supplemental-synthetic-observation' ? 'general' : 'lifecycle'
        }`,
      );
    }
  }
  if (command === 'login-marker') sensitiveArguments.push('--talking-quill-login-start');
  const startupFrame = Buffer.from(
    `${canonicalAcceptanceJson({ version: 1, arguments: sensitiveArguments })}\n`,
  );
  const child = options.spawnProcess(
    options.executable,
    ['--talking-quill-installed-acceptance-fd=3'],
    options.timeoutMs,
    startupFrame,
  );
  let rejectAbort;
  const aborted = new Promise((_, reject) => {
    rejectAbort = reject;
  });
  const abort = async () => {
    resultChannel.close();
    armedChannel?.close();
    try {
      await child.terminateIfRunning();
      rejectAbort(new Error('Packaged probe was actively cancelled after confirmed teardown'));
    } catch (error) {
      rejectAbort(error instanceof Error ? error : new Error('Packaged probe cancellation failed'));
    }
  };
  options.signal?.addEventListener('abort', abort, { once: true });
  if (options.signal?.aborted === true) await abort();
  const wait = (operation) => Promise.race([operation, aborted]);
  try {
    const armed = armedChannel === null ? null : await wait(armedChannel.value);
    if (armed !== null) {
      const expectedPhase =
        command === 'manual-physical-observation' ? 'observation-started' : 'armed';
      assertBoundResponse(armed, launchCorrelation, expectedPhase);
      if (command === 'manual-physical-observation') options.onObservationStarted?.(armed);
      const armedAction = await options.onArmed?.(armed, child);
      if (command === 'electron-crash-arm') {
        if (armedAction?.exitCode !== 0) throw new Error('Electron crash was not observed');
        await wait(child.exited.catch(() => undefined));
        return { result: 'passed', correlation: launchCorrelation, armed, oldElectronExited: true };
      }
    }
    const result = await wait(resultChannel.value);
    assertBoundResponse(result, launchCorrelation);
    await wait(child.exited);
    return result;
  } finally {
    options.signal?.removeEventListener('abort', abort);
    resultChannel.close();
    armedChannel?.close();
    await child.terminateIfRunning();
  }
}

function readFrozenSignedRequest(encoded, command, buildId, invocation, nowMs) {
  if (typeof encoded !== 'string' || !/^[A-Za-z0-9_-]+$/u.test(encoded)) {
    throw new Error('Frozen signed acceptance request is unavailable');
  }
  const canonical = Buffer.from(encoded, 'base64url');
  if (canonical.toString('base64url') !== encoded || canonical.length > 16 * 1024) {
    throw new Error('Frozen signed acceptance request is invalid');
  }
  const envelope = JSON.parse(canonical.toString('utf8'));
  if (
    Buffer.from(canonicalAcceptanceJson(envelope)).toString('base64url') !== encoded ||
    envelope?.payload?.command !== command ||
    envelope.payload.buildId !== buildId ||
    (invocation !== undefined &&
      (envelope.payload.invocationId !== invocation.invocationId ||
        envelope.payload.latestStartOffsetMs !== invocation.latestStartOffsetMs ||
        envelope.payload.deadlineOffsetMs !== invocation.deadlineOffsetMs)) ||
    !Number.isSafeInteger(envelope.payload.expiresAtMs) ||
    nowMs > envelope.payload.expiresAtMs ||
    nowMs < envelope.payload.runWindow?.notBeforeMs ||
    nowMs > envelope.payload.runWindow?.notBeforeMs + envelope.payload.latestStartOffsetMs ||
    typeof envelope.signatureBase64url !== 'string'
  ) {
    throw new Error('Frozen signed acceptance request binding is invalid');
  }
  return envelope;
}

export function createOneUseJsonChannel(pipeName, timeoutMs) {
  let settled = false;
  let connected = false;
  let timer;
  let resolveListening;
  let rejectListening;
  const listening = new Promise((resolvePromise, rejectPromise) => {
    resolveListening = resolvePromise;
    rejectListening = rejectPromise;
  });
  let resolveValue;
  let rejectValue;
  const value = new Promise((resolvePromise, rejectPromise) => {
    resolveValue = resolvePromise;
    rejectValue = rejectPromise;
  });
  const server = net.createServer((socket) => {
    if (connected) {
      socket.destroy();
      return;
    }
    connected = true;
    server.close();
    let bytes = Buffer.alloc(0);
    socket.on('data', (chunk) => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > MAX_RESPONSE_BYTES) socket.destroy(new Error('Probe response too large'));
    });
    socket.once('error', fail);
    socket.once('end', () => {
      try {
        const text = bytes.toString('utf8');
        if (!text.endsWith('\n') || text.indexOf('\n') !== text.length - 1) {
          throw new Error('Probe response must be one newline-terminated frame');
        }
        settled = true;
        clearTimeout(timer);
        resolveValue(JSON.parse(text));
      } catch (error) {
        fail(error);
      }
    });
  });
  const fail = (error) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    server.close();
    rejectValue(error);
  };
  server.once('error', (error) => {
    rejectListening(error);
    fail(error);
  });
  server.listen(pipeName, () => resolveListening());
  timer = setTimeout(() => fail(new Error('Packaged probe response timed out')), timeoutMs);
  return { listening, value, close: () => server.close() };
}

function assertBoundResponse(value, correlation, expectedPhase) {
  if (
    value === null ||
    typeof value !== 'object' ||
    value.version !== 1 ||
    value.correlation !== correlation ||
    value.runtimeLifecycleAuthoritative !== false ||
    (expectedPhase === undefined ? value.result !== 'passed' : value.phase !== expectedPhase)
  ) {
    throw new Error('Packaged probe response was not bound to its signed request');
  }
}

export function spawnPackagedProcess(executable, arguments_, timeoutMs, startupFrame) {
  if (
    startupFrame !== undefined &&
    (!Buffer.isBuffer(startupFrame) || startupFrame.length === 0 || startupFrame.length > 20 * 1024)
  ) {
    throw new Error('Packaged probe startup frame is invalid');
  }
  const child = spawn(executable, arguments_, {
    shell: false,
    windowsHide: true,
    stdio: startupFrame === undefined ? 'ignore' : ['ignore', 'ignore', 'ignore', 'pipe'],
    detached: false,
    env: sanitizedChildEnvironment(),
  });
  if (startupFrame !== undefined) {
    const startup = child.stdio[3];
    if (startup === null || startup === undefined) {
      child.kill('SIGKILL');
      throw new Error('Packaged probe startup pipe is unavailable');
    }
    startup.end(startupFrame);
  }
  let running = true;
  let rejectExit;
  const exited = new Promise((resolveExit, rejectPromise) => {
    rejectExit = rejectPromise;
    child.once('error', (error) => {
      running = false;
      rejectPromise(error);
    });
    child.once('exit', (code) => {
      running = false;
      if (code === 0) resolveExit(code);
      else rejectPromise(new Error(`Packaged probe exited ${String(code)}`));
    });
  });
  const timer = setTimeout(() => {
    void terminate().then(
      () => rejectExit(new Error('Packaged probe process timed out after confirmed teardown')),
      (error) => rejectExit(error),
    );
  }, timeoutMs);
  const terminate = async () => {
    if (!running || child.pid === undefined) return;
    if (process.platform === 'win32') {
      await new Promise((resolveTaskkill, rejectTaskkill) => {
        const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], {
          stdio: 'ignore',
          windowsHide: true,
          env: sanitizedChildEnvironment(),
        });
        const killerTimer = setTimeout(() => {
          killer.kill('SIGKILL');
          rejectTaskkill(new Error('Packaged probe retirement command timed out'));
        }, 5_000);
        killer.once('exit', (code) => {
          clearTimeout(killerTimer);
          if (code === 0 || code === 128) resolveTaskkill();
          else rejectTaskkill(new Error(`Packaged probe retirement exited ${String(code)}`));
        });
        killer.once('error', (error) => {
          clearTimeout(killerTimer);
          rejectTaskkill(error);
        });
      });
    } else child.kill('SIGKILL');
    const retired = await Promise.race([
      exited.then(
        () => true,
        () => true,
      ),
      delay(10_000).then(() => false),
    ]);
    if (!retired || running) throw new Error('Packaged probe remained alive after cancellation');
  };
  return {
    pid: child.pid,
    exited: exited.finally(() => clearTimeout(timer)),
    terminateIfRunning: terminate,
  };
}

function sanitizedChildEnvironment() {
  return sanitizedSubprocessEnvironment();
}

const delay = (milliseconds) =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
