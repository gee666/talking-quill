import { createPrivateKey, randomBytes, sign } from 'node:crypto';
import { spawn } from 'node:child_process';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { launchVerifiedChild } from './windows-verified-child-launcher.mjs';
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
  const armedExpectedPhase =
    command === 'manual-physical-observation' ? 'observation-started' : 'armed';
  const executableIdentity = options.executableIdentity;
  const brokerIdentity = options.brokerIdentity;
  if (
    executableIdentity?.path !== options.executable ||
    !/^[0-9a-f]{64}$/u.test(executableIdentity?.sha256 ?? '') ||
    !Number.isSafeInteger(executableIdentity?.bytes) ||
    !/^[0-9a-f]{40}$/u.test(options.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(options.sourceTree ?? '') ||
    !/^[0-9a-f]{64}$/u.test(brokerIdentity?.sha256 ?? '')
  ) {
    throw new Error('Native probe broker identities are invalid');
  }
  const startupFrame = Buffer.from(
    `${canonicalAcceptanceJson({ version: 1, signedRequest: options.signedRequest })}\n`,
  );
  const correlation = randomBytes(16).toString('hex');
  const child = (options.launchVerifiedChild ?? launchVerifiedChild)({
    bootstrap: options.bootstrapIdentity,
    child: brokerIdentity,
    timeoutMs: options.timeoutMs,
  });
  const events = createBrokerEventReader(child, correlation);
  child.stdin.write(
    `${canonicalAcceptanceJson({
      version: 1,
      operation: 'probe',
      correlation,
      brokerSha256: brokerIdentity.sha256,
      brokerBytes: brokerIdentity.bytes,
      executablePath: options.executable,
      executableSha256: executableIdentity.sha256,
      executableBytes: executableIdentity.bytes,
      sourceCommit: options.sourceCommit,
      sourceTree: options.sourceTree,
      startupFrameHex: startupFrame.toString('hex'),
      readinessPipe,
      armedPipe,
      armedExpectedPhase: armedPipe === null ? null : armedExpectedPhase,
      launchCorrelation,
      absoluteDeadlineMs: Date.now() + options.timeoutMs,
    })}\n`,
  );
  const termination = events.next('terminated');
  const waitEvent = (name) =>
    Promise.race([
      events.next(name),
      termination.then(() => {
        throw new Error('Packaged probe was actively cancelled after confirmed teardown');
      }),
    ]);
  const listening = await waitEvent('listening');
  let cancellationRequested = false;
  const control = {
    pid: listening.processId,
    exited: events.exited,
    terminateIfRunning: async () => {
      if (!cancellationRequested) {
        cancellationRequested = true;
        events.action('terminate');
      }
      await termination;
    },
  };
  const abort = () => void control.terminateIfRunning().catch(() => undefined);
  options.signal?.addEventListener('abort', abort, { once: true });
  try {
    if (options.signal?.aborted === true) {
      await control.terminateIfRunning();
      throw new Error('Packaged probe was actively cancelled after confirmed teardown');
    }
    if (armedPipe !== null) {
      const event = await waitEvent('armed');
      const armed = event.value;
      assertBoundResponse(armed, launchCorrelation, armedExpectedPhase);
      if (command === 'manual-physical-observation') options.onObservationStarted?.(armed);
      await options.onArmed?.(armed, control);
      if (cancellationRequested && command !== 'electron-crash-arm') {
        await termination;
        throw new Error('Packaged probe was actively cancelled after confirmed teardown');
      }
      if (command === 'electron-crash-arm') {
        await control.terminateIfRunning();
        return { result: 'passed', correlation: launchCorrelation, armed, oldElectronExited: true };
      }
      events.action('continue');
    }
    const complete = await waitEvent('complete');
    assertBoundResponse(complete.value, launchCorrelation);
    await events.exited;
    return complete.value;
  } finally {
    options.signal?.removeEventListener('abort', abort);
    if (!events.closed) child.kill('SIGKILL');
  }
}

function createBrokerEventReader(child, correlation) {
  let buffered = '';
  const waiting = new Map();
  const queued = new Map();
  let failed;
  let closed = false;
  const rejectAll = (error) => {
    failed = error;
    for (const entries of waiting.values()) for (const entry of entries) entry.reject(error);
    waiting.clear();
  };
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    buffered += chunk;
    if (buffered.length > 128 * 1024)
      return rejectAll(new Error('Probe broker output exceeded its bound'));
    for (;;) {
      const newline = buffered.indexOf('\n');
      if (newline < 0) break;
      const line = buffered.slice(0, newline);
      buffered = buffered.slice(newline + 1);
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        rejectAll(new Error('Probe broker output is invalid'));
        return;
      }
      const keys = Object.keys(event ?? {})
        .sort()
        .join(',');
      const expectedKeys =
        event?.value === undefined
          ? 'correlation,event,processId,rejectedClients,version'
          : 'correlation,event,processId,rejectedClients,value,version';
      if (
        keys !== expectedKeys ||
        event.version !== 1 ||
        event.correlation !== correlation ||
        !['listening', 'armed', 'complete', 'terminated'].includes(event.event) ||
        !Number.isSafeInteger(event.processId) ||
        event.processId <= 0 ||
        !Number.isSafeInteger(event.rejectedClients) ||
        event.rejectedClients < 0 ||
        (['armed', 'complete'].includes(event.event) &&
          (event.value === null || typeof event.value !== 'object'))
      ) {
        rejectAll(new Error('Probe broker event is malformed or uncorrelated'));
        return;
      }
      const entry = waiting.get(event.event)?.shift();
      if (entry === undefined) {
        const entries = queued.get(event.event) ?? [];
        entries.push(event);
        queued.set(event.event, entries);
      } else entry.resolve(event);
    }
  });
  const stderr = [];
  child.stderr.on('data', (chunk) => {
    if (stderr.reduce((n, value) => n + value.length, 0) < 4096) stderr.push(chunk);
  });
  const exited = new Promise((resolveExit, rejectExit) =>
    child.once('exit', (code, signal) => {
      closed = true;
      if (code === 0 && signal === null && buffered === '') resolveExit(code);
      else rejectExit(new Error('Native probe broker failed'));
    }),
  );
  exited.catch(rejectAll);
  return {
    get closed() {
      return closed;
    },
    exited,
    next(event) {
      if (failed !== undefined) return Promise.reject(failed);
      const queuedEvent = queued.get(event)?.shift();
      if (queuedEvent !== undefined) return Promise.resolve(queuedEvent);
      return new Promise((resolveEvent, reject) => {
        const entries = waiting.get(event) ?? [];
        entries.push({ resolve: resolveEvent, reject });
        waiting.set(event, entries);
      });
    },
    action(action) {
      if (child.stdin.destroyed) throw new Error('Probe broker control channel is closed');
      child.stdin.write(`${canonicalAcceptanceJson({ version: 1, correlation, action })}\n`);
    },
  };
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

function isBoundResponse(value, correlation, expectedPhase) {
  return (
    value !== null &&
    typeof value === 'object' &&
    value.version === 1 &&
    value.correlation === correlation &&
    value.runtimeLifecycleAuthoritative === false &&
    (expectedPhase === undefined ? value.result === 'passed' : value.phase === expectedPhase)
  );
}

function assertBoundResponse(value, correlation, expectedPhase) {
  if (!isBoundResponse(value, correlation, expectedPhase)) {
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
    child.kill('SIGKILL');
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
