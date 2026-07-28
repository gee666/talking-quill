import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { createInterface } from 'node:readline/promises';
import { stdin as input, stdout as output } from 'node:process';

const arguments_ = process.argv.slice(2);
const interactive = arguments_.includes('--interactive');
const helperArgument = valueAfter('--helper');
const helper = resolve(
  helperArgument ??
    `app/native/${process.platform === 'win32' ? 'talking-quill-helper.exe' : 'talking-quill-helper'}`,
);
if (!isAbsolute(helper)) throw new Error('Helper path must resolve to an absolute path');
await access(helper);

const child = spawn(helper, [], {
  stdio: ['pipe', 'pipe', 'inherit'],
  shell: false,
  windowsHide: false,
  env: { NO_COLOR: '1' },
});
const childExit = new Promise((resolveExit, reject) => {
  child.once('error', reject);
  child.once('exit', (code) => (code === 0 ? resolveExit() : reject(new Error(`exit ${code}`))));
});
let pendingBytes = Buffer.alloc(0);
let nextId = 1;
const pending = new Map();
const notifications = [];

child.stdout.on('data', (chunk) => {
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
      console.log(`event ${JSON.stringify(message)}`);
    }
  }
});

const initialized = await request('initialize', { protocolVersion: 5 });
await request('activation.configure', { enabled: false, bindings: [] });
await request('session.set_capture', { active: false });
const permissions = await request('permissions.get', {});
const activationRegistration = await configureActivationCoverage(initialized, permissions);
await request('activation.configure', { enabled: false, bindings: [] });
const safeReport = {
  initialized,
  activationRegistration,
  health: await request('ping', {}),
  permissions,
  frontApp: await request('front_app.get', {}).catch((error) => ({ unavailable: error.message })),
};
console.log(JSON.stringify(safeReport, null, 2));

if (interactive) await runInteractive();
await request('session.set_capture', { active: false }).catch(() => undefined);
await request('activation.configure', { enabled: false, bindings: [] }).catch(() => undefined);
await request('shutdown', {});
await childExit;

async function configureActivationCoverage(initialization, permissions) {
  const permissionReady = Object.values(permissions).every((value) =>
    ['granted', 'not_applicable'].includes(value),
  );
  if (initialization.hookStatus !== 'ready' || !permissionReady) {
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
        ? 'For 20 seconds, focus the editor and perform Alt+X, Alt+X+P, Alt+X+Q, Alt+X+M, and Alt+X+T. Keep X held while pressing each suffix, release all keys between chords, and release the suffix before X. X/modifiers may leak; final triggers must not.'
        : 'For 20 seconds, focus the editor and perform Option+X, Option+X+P, Option+X+Q, Option+X+M, and Option+X+T. Keep X held while pressing each suffix, release all keys between chords, and release the suffix before X. X/modifiers may leak; final triggers must not.',
    );
    await delay(20_000);
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
    await request('session.set_capture', { active: true });
    console.log(
      'For 15 seconds, focus the editor and press Esc and Enter. Both should be absent there.',
    );
    await delay(15_000);
    await request('session.set_capture', { active: false });
    const sessionKeys = printObserved('session.key');
    assertPairedEvents(sessionKeys, 'session.key');
    for (const key of ['escape', 'enter']) {
      if (!sessionKeys.some((event) => event.params.key === key && event.params.phase === 'down')) {
        throw new Error(`No ${key} session-control event was observed`);
      }
    }

    await terminal.question(
      'Copy distinctive Unicode/multiline text, focus Notepad or TextEdit, then return here and press Enter. ',
    );
    console.log('Switch back to the target in the next three seconds.');
    await delay(3_000);
    console.log(`front app before paste: ${JSON.stringify(await request('front_app.get', {}))}`);
    console.log(`paste dispatch: ${JSON.stringify(await request('paste.inject', {}))}`);
    await terminal.question('Verify the exact clipboard text appeared once, then press Enter. ');
  } finally {
    terminal.close();
  }
}

function printObserved(method) {
  const observed = notifications.filter((notification) => notification.method === method);
  console.log(`${method}: ${String(observed.length)} event(s) ${JSON.stringify(observed)}`);
  return observed;
}

function assertPairedEvents(events, label) {
  const counts = new Map();
  for (const event of events) {
    const identity = JSON.stringify({
      profileId: event.params.profileId,
      shortcut: event.params.shortcut,
      ...(event.params.key === undefined ? {} : { key: event.params.key }),
    });
    const count = counts.get(identity) ?? { down: 0, up: 0, complete: 0 };
    count[event.params.phase] += 1;
    counts.set(identity, count);
  }
  if (
    counts.size === 0 ||
    [...counts.values()].some(
      (count) => count.complete === 0 && (count.down === 0 || count.down !== count.up),
    )
  ) {
    throw new Error(`${label} did not contain exact paired down/up events`);
  }
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
  const id = nextId++;
  const payload = Buffer.from(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  if (payload.length === 0 || payload.length > 16 * 1024) throw new Error('Request too large');
  const frame = Buffer.allocUnsafe(payload.length + 4);
  frame.writeUInt32BE(payload.length, 0);
  payload.copy(frame, 4);
  return new Promise((resolveRequest, reject) => {
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out`));
    }, 3_000);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timeout);
        resolveRequest(value);
      },
      reject: (error) => {
        clearTimeout(timeout);
        reject(error);
      },
    });
    child.stdin.write(frame);
  });
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
