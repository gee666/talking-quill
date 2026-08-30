import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { createInterface } from 'node:readline/promises';
import { stdin as input, stdout as output } from 'node:process';

import {
  FAILURE_CLEANUP_REQUESTS,
  prepareHelperHarnessExecutable,
} from './helper-harness-support.mjs';

const arguments_ = process.argv.slice(2);
const interactive = arguments_.includes('--interactive');
const helperArgument = valueAfter('--helper');
const repositoryRoot = resolve(import.meta.dirname, '..', '..');
const sourceHelper = resolve(
  helperArgument ??
    `app/native/${process.platform === 'win32' ? 'talking-quill-helper.exe' : 'talking-quill-helper'}`,
);
if (!isAbsolute(sourceHelper)) throw new Error('Helper path must resolve to an absolute path');
await access(sourceHelper);
const preparedHelper = await prepareHelperHarnessExecutable({
  helper: sourceHelper,
  repositoryRoot,
});

const child = spawn(
  preparedHelper.executable,
  preparedHelper.staged ? ['--windows-helper-harness-v1'] : [],
  {
    stdio: ['pipe', 'pipe', 'inherit'],
    shell: false,
    windowsHide: false,
    env: { ...process.env, NO_COLOR: '1' },
  },
);
const childExit = new Promise((resolveExit, reject) => {
  child.once('error', reject);
  child.once('exit', (code) => {
    const error = code === 0 ? null : new Error(`helper exited with code ${String(code)}`);
    for (const request of pending.values()) {
      request.reject(error ?? new Error('helper exited before responding'));
    }
    pending.clear();
    if (error === null) resolveExit();
    else reject(error);
  });
});
// Observe spawn failures immediately while preserving rejection for the main
// flow, which reports the original error after bounded cleanup.
void childExit.catch(() => undefined);
let pendingBytes = Buffer.alloc(0);
let nextId = 1;
const pending = new Map();
const notifications = [];
const requestSequence = [];

child.stdin.on('error', () => undefined);
child.stdout.on('data', (chunk) => {
  try {
    pendingBytes = Buffer.concat([pendingBytes, chunk]);
    while (pendingBytes.length >= 4) {
      const length = pendingBytes.readUInt32BE(0);
      if (length === 0 || length > 16 * 1024) throw new Error(`Invalid frame length ${length}`);
      if (pendingBytes.length < length + 4) return;
      const message = JSON.parse(pendingBytes.subarray(4, length + 4).toString('utf8'));
      pendingBytes = pendingBytes.subarray(length + 4);
      if ('id' in message) {
        const request = pending.get(message.id);
        if (request === undefined) throw new Error(`Unknown response ID ${String(message.id)}`);
        pending.delete(message.id);
        if ('error' in message) request.reject(new Error(message.error.message));
        else request.resolve(message.result);
      } else {
        notifications.push(message);
        console.log(`event ${JSON.stringify(redactNotification(message))}`);
      }
    }
  } catch (error) {
    const protocolError = error instanceof Error ? error : new Error(String(error));
    for (const request of pending.values()) request.reject(protocolError);
    pending.clear();
  }
});

let protocolInitialized = false;
let authenticatedOwnerInstance = null;
let plannedShutdown = false;
let initializeResponseComplete;
const initializeResponse = new Promise((resolveResponse) => {
  initializeResponseComplete = resolveResponse;
});
let runError;
try {
  const initialized = await request('initialize', { protocolVersion: 10 });
  validateKeyboardOwnerSnapshot(initialized.keyboardOwner);
  authenticatedOwnerInstance = initialized.keyboardOwner.instanceId;
  validateKeyboardCaptureCapability(initialized.keyboardCapture, initialized.keyboardOwner);
  if (preparedHelper.staged && !initialized.keyboardOwner.authenticated) {
    throw new Error('Staged Windows helper did not authenticate its provenance-bound owner');
  }
  const permissions = await request('permissions.get', {});
  const health = await request('ping', {});
  validateKeyboardOwnerSnapshot(health.keyboardOwner);
  if (
    health.keyboardOwner.instanceId !== initialized.keyboardOwner.instanceId ||
    health.keyboardOwner.leaseEpoch !== initialized.keyboardOwner.leaseEpoch
  ) {
    throw new Error('Helper ping changed keyboard-owner instance or lease epoch');
  }
  await request('session.set_capture', { mode: 'off' });
  await request('activation.configure', { enabled: false, bindings: fullChordBindings() });
  if (
    JSON.stringify(requestSequence.slice(0, 5)) !==
    JSON.stringify([
      'initialize',
      'permissions.get',
      'ping',
      'session.set_capture',
      'activation.configure',
    ])
  ) {
    throw new Error(`Helper disabled-first startup order changed: ${requestSequence.join(' -> ')}`);
  }
  const activationRegistration = await configureActivationCoverage(initialized, permissions);
  await request('activation.configure', { enabled: false, bindings: [] });
  const observability = await request('runtime.observability', {});
  validateKeyboardOwnerSnapshot(observability.keyboardOwner);
  if (typeof observability.owner?.authAttempts !== 'number') {
    throw new Error(`Malformed owner observability: ${JSON.stringify(observability.owner)}`);
  }
  const maintenanceBoundary = await validateMaintenanceBoundary(initialized.keyboardOwner);
  const safeReport = {
    initialized,
    activationRegistration,
    health,
    permissions,
    observability,
    maintenanceBoundary,
    frontApp: await request('front_app.get', {}).catch((error) => ({ unavailable: error.message })),
  };
  console.log(JSON.stringify(safeReport, null, 2));

  if (interactive) {
    if (activationRegistration.safeDisabled) {
      throw new Error('Interactive activation checks require a trusted native test build');
    }
    await runInteractive();
  }
  const shutdown = await requestDisabledNeutralShutdown();
  if (!['neutral', 'draining'].includes(shutdown.ownerDisposition)) {
    throw new Error(`Malformed owner shutdown disposition: ${JSON.stringify(shutdown)}`);
  }
  plannedShutdown = true;
  if (!(await waitForChildExit(3_000))) {
    throw new Error('Helper did not exit after its planned neutral shutdown response');
  }
  await childExit;
} catch (error) {
  runError = error;
} finally {
  if (!plannedShutdown && child.exitCode === null) {
    await cleanupFailedHarness();
  }
  let childExited = await waitForChildExit(3_000);
  if (!childExited && preparedHelper.staged) {
    try {
      await cleanupWithFreshGateway(preparedHelper.executable, authenticatedOwnerInstance);
    } catch (error) {
      runError ??= error;
    }
    childExited = await waitForChildExit(3_000);
  }
  if (!childExited) {
    runError ??= new Error(
      'Helper remained alive after fresh authenticated cleanup; retaining its process and staged package',
    );
  } else {
    try {
      await preparedHelper.cleanup();
    } catch (error) {
      runError ??= error;
    }
  }
}
if (runError !== undefined) throw runError;

async function waitForChildExit(milliseconds) {
  if (child.exitCode !== null) return true;
  return Promise.race([
    childExit.then(
      () => true,
      () => true,
    ),
    delay(milliseconds).then(() => false),
  ]);
}

async function requestDisabledNeutralShutdown() {
  let shutdown;
  for (const [method, params] of FAILURE_CLEANUP_REQUESTS) {
    const result = await request(method, params);
    if (method === 'shutdown') shutdown = result;
  }
  return shutdown;
}

async function cleanupFailedHarness() {
  if (!protocolInitialized) {
    await Promise.race([initializeResponse, delay(3_500)]);
  }
  if (protocolInitialized) {
    try {
      await requestDisabledNeutralShutdown();
      plannedShutdown = true;
      return;
    } catch {
      // The requests were issued in disabled-first order. Never force-kill an
      // owner whose native neutrality the harness cannot authenticate.
    }
  }
  for (const [method, params] of FAILURE_CLEANUP_REQUESTS) {
    sendWithoutWaiting(method, params);
  }
  child.stdin.end();
}

async function cleanupWithFreshGateway(executable, expectedOwnerInstance) {
  const cleanup = spawn(executable, [], {
    stdio: ['pipe', 'pipe', 'inherit'],
    shell: false,
    windowsHide: false,
    env: { ...process.env, NO_COLOR: '1' },
  });
  cleanup.stdin.on('error', () => undefined);
  let bytes = Buffer.alloc(0);
  let id = 1;
  const requests = new Map();
  const exit = new Promise((resolveExit, rejectExit) => {
    cleanup.once('error', rejectExit);
    cleanup.once('exit', (code) => {
      const error =
        code === 0 ? null : new Error(`cleanup gateway exited with code ${String(code)}`);
      for (const request of requests.values()) {
        request.reject(error ?? new Error('cleanup gateway exited before responding'));
      }
      requests.clear();
      if (error === null) resolveExit();
      else rejectExit(error);
    });
  });
  void exit.catch(() => undefined);
  cleanup.stdout.on('data', (chunk) => {
    bytes = Buffer.concat([bytes, chunk]);
    while (bytes.length >= 4) {
      const length = bytes.readUInt32BE(0);
      if (length === 0 || length > 16 * 1024 || bytes.length < length + 4) return;
      const message = JSON.parse(bytes.subarray(4, length + 4).toString('utf8'));
      bytes = bytes.subarray(length + 4);
      if (!('id' in message)) continue;
      const pendingRequest = requests.get(message.id);
      if (pendingRequest === undefined) throw new Error('Unknown cleanup gateway response');
      requests.delete(message.id);
      if ('error' in message) pendingRequest.reject(new Error(message.error.message));
      else pendingRequest.resolve(message.result);
    }
  });
  const requestCleanup = (method, params) => {
    const requestId = id++;
    const payload = Buffer.from(JSON.stringify({ jsonrpc: '2.0', id: requestId, method, params }));
    const frame = Buffer.allocUnsafe(payload.length + 4);
    frame.writeUInt32BE(payload.length, 0);
    payload.copy(frame, 4);
    return new Promise((resolveRequest, rejectRequest) => {
      const timeout = setTimeout(() => {
        requests.delete(requestId);
        rejectRequest(new Error(`fresh cleanup ${method} timed out`));
      }, 3_000);
      requests.set(requestId, {
        resolve: (value) => {
          clearTimeout(timeout);
          resolveRequest(value);
        },
        reject: (error) => {
          clearTimeout(timeout);
          rejectRequest(error);
        },
      });
      cleanup.stdin.write(frame, (error) => {
        if (error) rejectRequest(error);
      });
    });
  };

  try {
    const initialized = await requestCleanup('initialize', { protocolVersion: 10 });
    validateKeyboardOwnerSnapshot(initialized.keyboardOwner);
    if (
      expectedOwnerInstance !== null &&
      initialized.keyboardOwner.instanceId !== expectedOwnerInstance
    ) {
      throw new Error('Fresh cleanup gateway authenticated a different owner instance');
    }
    for (const [method, params] of FAILURE_CLEANUP_REQUESTS) {
      await requestCleanup(method, params);
    }
    if (!(await Promise.race([exit.then(() => true), delay(3_000).then(() => false)]))) {
      throw new Error('Fresh cleanup gateway did not exit after planned neutral shutdown');
    }
    await exit;
  } catch (error) {
    cleanup.stdin.end();
    await Promise.race([exit.catch(() => undefined), delay(3_000)]);
    throw error;
  }
}

function validateKeyboardOwnerSnapshot(owner) {
  if (
    owner?.model !== 'out_of_process' ||
    owner.protocolVersion !== 1 ||
    ![
      'safe_disabled',
      'idle',
      'leased_disabled',
      'leased_enabled',
      'draining',
      'maintenance',
      'degraded',
      'unavailable',
    ].includes(owner.state) ||
    typeof owner.instanceId !== 'string' ||
    typeof owner.buildId !== 'string' ||
    typeof owner.authenticated !== 'boolean' ||
    (owner.leaseEpoch !== null && (!Number.isSafeInteger(owner.leaseEpoch) || owner.leaseEpoch < 1))
  ) {
    throw new Error(`Malformed keyboardOwner snapshot: ${JSON.stringify(owner)}`);
  }
  if (owner.authenticated && (!owner.instanceId || !owner.buildId || owner.leaseEpoch === null)) {
    throw new Error(
      `Authenticated keyboardOwner has incomplete identity: ${JSON.stringify(owner)}`,
    );
  }
}

async function validateMaintenanceBoundary(owner) {
  if (owner.authenticated) {
    return {
      skipped:
        'Authenticated maintenance is exercised only by the R8 installed coordinator to avoid sealing an interactive native harness owner.',
    };
  }
  try {
    await request('owner.prepare_maintenance', {
      operation: 'uninstall',
      transactionId: 'a'.repeat(64),
      sourceBuildId: 'b'.repeat(64),
    });
  } catch (error) {
    return { failClosed: true, message: error instanceof Error ? error.message : String(error) };
  }
  throw new Error('Unavailable owner unexpectedly accepted maintenance preparation');
}

function validateKeyboardCaptureCapability(capability, owner) {
  if (
    capability.activationAvailable !== capability.sessionKeyCaptureAvailable ||
    (capability.activationAvailable &&
      (capability.buildDisabled ||
        capability.runtimeRollbackActive ||
        !owner.authenticated ||
        owner.leaseEpoch === null ||
        owner.state !== 'leased_disabled'))
  ) {
    throw new Error(
      `Malformed owner-gated keyboardCapture handshake: ${JSON.stringify({ capability, owner })}`,
    );
  }
}

async function configureActivationCoverage(initialization, permissions) {
  const permissionReady = Object.values(permissions).every((value) =>
    ['granted', 'not_applicable'].includes(value),
  );
  if (initialization.keyboardCapture.buildDisabled) {
    const rejected = await request('activation.configure', {
      enabled: true,
      bindings: fullChordBindings(),
    });
    if (rejected.enabled !== false || rejected.bindings.length !== fullChordBindings().length) {
      throw new Error(
        `Safe-disabled helper accepted activation configuration: ${JSON.stringify(rejected)}`,
      );
    }
    return {
      safeDisabled: true,
      configuration: rejected,
      verified: 'Build-disabled protocol behavior rejected activation enablement.',
    };
  }
  if (initialization.hookStatus !== 'installed_unobserved' || !permissionReady) {
    return {
      skipped:
        'Native hook or permissions unavailable; full-chord runtime coverage requires an interactive trusted host.',
    };
  }
  const expectedBindings = fullChordBindings();
  const configuration = await request('activation.configure', {
    enabled: true,
    bindings: expectedBindings,
  });
  if (
    configuration.enabled !== true ||
    configuration.bindings.length !== expectedBindings.length ||
    expectedBindings.some((expected, index) => {
      const actual = configuration.bindings[index];
      return (
        actual?.profileId !== expected.profileId ||
        !shortcutMatches(actual.shortcut, expected.shortcut.keys, expected.shortcut.modifiers)
      );
    })
  ) {
    throw new Error(
      `Native helper did not round-trip all 13 profile bindings exactly: ${JSON.stringify({ expectedBindings, actualBindings: configuration.bindings })}`,
    );
  }
  return {
    configuredChords:
      process.platform === 'win32'
        ? ['Alt+KeyX', 'Alt+KeyX+KeyP', 'Alt+KeyX+KeyQ', 'Alt+KeyX+KeyM', 'Alt+KeyX+KeyT']
        : [
            'Option+KeyX',
            'Option+KeyX+KeyP',
            'Option+KeyX+KeyQ',
            'Option+KeyX+KeyM',
            'Option+KeyX+KeyT',
          ],
    bindingCount: configuration.bindings.length,
    configuration,
  };
}

async function runInteractive() {
  const terminal = createInterface({ input, output });
  try {
    await terminal.question(
      'Focus a test editor and verify Enter/Esc type normally. Return here and press Enter to continue. ',
    );
    notifications.length = 0;
    const windows = process.platform === 'win32';
    await request('activation.configure', {
      enabled: true,
      bindings: fullChordBindings(),
    });
    console.log(
      windows
        ? 'For 20 seconds, focus the editor and perform Alt+X, Alt+X+P, Alt+X+Q, Alt+X+M, and Alt+X+T. Keep X held while pressing each suffix, release all keys between chords, and release the suffix before X. Every captured candidate character must remain absent and modifier edges must remain balanced.'
        : 'For 20 seconds, focus the editor and perform Option+X, Option+X+P, Option+X+Q, Option+X+M, and Option+X+T. Keep X held while pressing each suffix, release all keys between chords, and release the suffix before X. Every captured candidate character must remain absent and modifier edges must remain balanced.',
    );
    await delay(20_000);
    const capturedCharactersAbsent = await terminal.question(
      'Verify the editor contains none of the captured candidate characters; type ABSENT to assert this: ',
    );
    if (capturedCharactersAbsent.trim() !== 'ABSENT') {
      throw new Error('Interactive native run did not assert captured characters were absent');
    }
    await request('activation.configure', { enabled: false, bindings: [] });
    const activations = printObserved('activation.event');
    assertPairedEvents(activations, 'activation.event');
    const activationStarts = activations.filter((event) =>
      ['down', 'complete'].includes(event.params.phase),
    );
    const expectedShortcuts = [
      ['general', ['X'], { ctrl: false, alt: true, shift: false, meta: false }],
      ['prompt', ['X', 'P'], { ctrl: false, alt: true, shift: false, meta: false }],
      ['prompt-to-english', ['X', 'Q'], { ctrl: false, alt: true, shift: false, meta: false }],
      ['markdown', ['X', 'M'], { ctrl: false, alt: true, shift: false, meta: false }],
      ['translate-to-english', ['X', 'T'], { ctrl: false, alt: true, shift: false, meta: false }],
    ];
    for (const [profileId, keys, modifiers] of expectedShortcuts) {
      if (
        !activationStarts.some(
          (event) =>
            event.params.profileId === profileId &&
            shortcutMatches(event.params.shortcut, keys, modifiers),
        )
      ) {
        throw new Error(
          `No ${JSON.stringify({ profileId, keys, modifiers })} activation start event was observed`,
        );
      }
    }

    notifications.length = 0;
    await request('session.set_capture', { mode: 'recording' });
    console.log(
      'For 15 seconds, focus the editor and press Esc and Enter. Both should be absent there.',
    );
    await delay(15_000);
    const recordingKeys = printObserved('session.key');
    assertPairedEvents(recordingKeys, 'recording session.key');
    for (const key of ['escape', 'enter']) {
      if (
        !recordingKeys.some((event) => event.params.key === key && event.params.phase === 'down')
      ) {
        throw new Error(`No ${key} recording session-control event was observed`);
      }
    }

    notifications.length = 0;
    await request('session.set_capture', { mode: 'cancel-only' });
    console.log(
      'For 15 seconds, focus the editor and press Esc and Enter. Esc should be absent; Enter should type normally.',
    );
    await delay(15_000);
    await request('session.set_capture', { mode: 'off' });
    const cancelOnlyKeys = printObserved('session.key');
    assertPairedEvents(cancelOnlyKeys, 'cancel-only session.key');
    if (
      !cancelOnlyKeys.some(
        (event) => event.params.key === 'escape' && event.params.phase === 'down',
      )
    ) {
      throw new Error('No Escape cancel-only session-control event was observed');
    }
    if (cancelOnlyKeys.some((event) => event.params.key === 'enter')) {
      throw new Error('Enter was captured in cancel-only mode');
    }

    const expectedPasteText = await terminal.question(
      'Enter the exact distinctive Unicode text you will copy for the paste check: ',
    );
    if (expectedPasteText.length === 0) throw new Error('Paste check text must not be empty');
    await terminal.question(
      `Copy this exact text, then press Enter: ${JSON.stringify(expectedPasteText)} `,
    );
    notifications.length = 0;
    await request('activation.configure', {
      enabled: true,
      bindings: fullChordBindings(),
    });
    console.log('Focus the paste target and perform the Prompt shortcut now.');
    await delay(10_000);
    await request('activation.configure', { enabled: false, bindings: [] });
    const activation = notifications
      .filter(
        (event) =>
          event.method === 'activation.event' &&
          event.params.profileId === 'prompt' &&
          ['down', 'complete'].includes(event.params.phase),
      )
      .at(-1)?.params;
    if (activation === undefined) throw new Error('No Prompt activation was observed');
    console.log(`front app before paste: ${JSON.stringify(await request('front_app.get', {}))}`);
    let expectedManualOutcome;
    if (activation.targetToken === null) {
      console.log(
        'No strong native target token was available; this control is intentionally clipboard-only.',
      );
      expectedManualOutcome = 'CLIPBOARD_ONLY';
    } else {
      const expectedClipboardSha256 = createHash('sha256')
        .update(expectedPasteText, 'utf8')
        .digest('hex');
      const pasteResult = await request('paste.inject', {
        activationGeneration: activation.activationGeneration,
        targetToken: activation.targetToken,
        expectedClipboardSha256,
      });
      console.log(`paste dispatch: ${JSON.stringify(pasteResult)}`);
      if (typeof pasteResult.submitted !== 'boolean') {
        throw new Error('paste.inject returned an invalid result');
      }
      expectedManualOutcome = pasteResult.submitted
        ? 'EXACT_ONCE'
        : pasteResult.reason === 'indeterminate'
          ? 'INDETERMINATE'
          : 'CLIPBOARD_ONLY';
    }
    const pasteAssertion = await terminal.question(
      `Type ${expectedManualOutcome} to confirm the authority-consistent observed outcome: `,
    );
    if (pasteAssertion.trim() !== expectedManualOutcome) {
      throw new Error(
        `Interactive paste assertion contradicted native authority; expected ${expectedManualOutcome}`,
      );
    }
  } finally {
    terminal.close();
  }
}

function printObserved(method) {
  const observed = notifications.filter((notification) => notification.method === method);
  console.log(
    `${method}: ${String(observed.length)} event(s) ${JSON.stringify(observed.map(redactNotification))}`,
  );
  return observed;
}

function redactNotification(notification) {
  if (notification.method !== 'activation.event' || notification.params.targetToken === null) {
    return notification;
  }
  return {
    ...notification,
    params: { ...notification.params, targetToken: '<redacted>' },
  };
}

function assertPairedEvents(events, label) {
  if (events.length === 0) {
    throw new Error(`${label} did not contain events`);
  }
  if (events.every((event) => event.method === 'activation.event')) {
    assertPairedActivationEvents(events, label);
    return;
  }

  const active = new Set();
  for (const event of events) {
    const identity = JSON.stringify({ key: event.params.key });
    if (event.params.phase === 'down') {
      if (active.has(identity)) throw new Error(`${label} contained a repeated/orphan down`);
      active.add(identity);
    } else if (event.params.phase === 'up') {
      if (!active.delete(identity)) throw new Error(`${label} contained an up before down`);
    } else {
      throw new Error(`${label} contained an unknown phase`);
    }
  }
  if (active.size !== 0) throw new Error(`${label} did not contain an up for every down`);
}

function assertPairedActivationEvents(events, label) {
  let lastGeneration = 0;
  let active = null;
  for (const event of events) {
    const { activationGeneration, phase, targetToken } = event.params;
    const validToken =
      targetToken === null ||
      (typeof targetToken === 'string' &&
        targetToken.length > 0 &&
        Buffer.byteLength(targetToken, 'utf8') <= 64);
    if (
      !Number.isSafeInteger(activationGeneration) ||
      activationGeneration < 1 ||
      !Object.hasOwn(event.params, 'targetToken') ||
      !validToken
    ) {
      throw new Error(`${label} contained an invalid v8 activation context`);
    }

    if (phase === 'down' || phase === 'complete') {
      if (activationGeneration <= lastGeneration || active !== null) {
        throw new Error(`${label} contained a non-monotonic or overlapping activation`);
      }
      lastGeneration = activationGeneration;
      if (phase === 'down') active = event.params;
      continue;
    }

    if (phase !== 'up' || active === null || !sameActivationIdentity(active, event.params)) {
      throw new Error(`${label} contained an orphan or mismatched up`);
    }
    active = null;
  }
  if (active !== null) throw new Error(`${label} did not contain an up for every down`);
}

function sameActivationIdentity(left, right) {
  return (
    left.activationGeneration === right.activationGeneration &&
    left.targetToken === right.targetToken &&
    left.profileId === right.profileId &&
    JSON.stringify(left.shortcut) === JSON.stringify(right.shortcut)
  );
}

function fullChordBindings() {
  return [
    binding('general', ['X'], { ctrl: false, alt: true, shift: false, meta: false }),
    binding('prompt', ['X', 'P'], { ctrl: false, alt: true, shift: false, meta: false }),
    binding('prompt-to-english', ['X', 'Q'], {
      ctrl: false,
      alt: true,
      shift: false,
      meta: false,
    }),
    binding('markdown', ['X', 'M'], { ctrl: false, alt: true, shift: false, meta: false }),
    binding('translate-to-english', ['X', 'T'], {
      ctrl: false,
      alt: true,
      shift: false,
      meta: false,
    }),
    ...Array.from({ length: 8 }, (_, index) =>
      binding(
        `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
        [String.fromCharCode('A'.charCodeAt(0) + index)],
        { ctrl: true, alt: true, shift: false, meta: false },
      ),
    ),
  ];
}

function binding(profileId, keys, modifiers) {
  return { profileId, shortcut: { modifiers, keys } };
}

function shortcutMatches(shortcut, keys, modifiers) {
  return (
    JSON.stringify(shortcut.keys) === JSON.stringify(keys) &&
    Object.entries(modifiers).every(([name, enabled]) => shortcut.modifiers[name] === enabled)
  );
}

function request(method, params) {
  requestSequence.push(method);
  const { id, frame } = requestFrame(method, params);
  return new Promise((resolveRequest, reject) => {
    const timeout = setTimeout(
      () => {
        if (method !== 'initialize') pending.delete(id);
        reject(new Error(`${method} timed out`));
      },
      method === 'initialize' ? 12_000 : 3_000,
    );
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timeout);
        if (method === 'initialize') {
          protocolInitialized = true;
          initializeResponseComplete();
        }
        resolveRequest(value);
      },
      reject: (error) => {
        clearTimeout(timeout);
        if (method === 'initialize') initializeResponseComplete();
        reject(error);
      },
    });
    child.stdin.write(frame, (error) => {
      if (error) {
        pending.delete(id);
        clearTimeout(timeout);
        if (method === 'initialize') initializeResponseComplete();
        reject(error);
      }
    });
  });
}

function sendWithoutWaiting(method, params) {
  requestSequence.push(method);
  const { frame } = requestFrame(method, params);
  child.stdin.write(frame, () => undefined);
}

function requestFrame(method, params) {
  const id = nextId++;
  const payload = Buffer.from(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  if (payload.length === 0 || payload.length > 16 * 1024) throw new Error('Request too large');
  const frame = Buffer.allocUnsafe(payload.length + 4);
  frame.writeUInt32BE(payload.length, 0);
  payload.copy(frame, 4);
  return { id, frame };
}

function valueAfter(name) {
  const index = arguments_.indexOf(name);
  if (index === -1) return null;
  const value = arguments_[index + 1];
  if (value === undefined || value.startsWith('--')) throw new Error(`${name} needs a value`);
  return value;
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}
