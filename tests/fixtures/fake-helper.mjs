const scenario = process.argv[2] ?? 'normal';
const platform = process.platform === 'win32' ? 'windows' : 'macos';
const architecture = process.arch === 'x64' ? 'x86_64' : 'aarch64';
let pending = Buffer.alloc(0);
let configured = false;
let initialized = false;

if (scenario === 'exit') process.exit(23);

process.stdin.on('data', (chunk) => {
  pending = Buffer.concat([pending, chunk]);
  while (pending.length >= 4) {
    const length = pending.readUInt32BE(0);
    if (length === 0 || length > 16 * 1024) process.exit(24);
    if (pending.length < length + 4) return;
    const payload = pending.subarray(4, length + 4);
    pending = pending.subarray(length + 4);
    handle(JSON.parse(payload.toString('utf8')));
  }
});

function handle(request) {
  if (scenario === 'timeout') return;
  if (scenario === 'malformed') {
    process.stdout.write(frame(Buffer.from('{invalid', 'utf8')));
    return;
  }
  const { id, method, params } = request;
  if (method === 'initialize') {
    if (params.protocolVersion !== 3) process.exit(28);
    const initialize = () => {
      respond(id, {
        protocolVersion: 3,
        helperVersion: scenario === 'mismatch' ? '9.9.9' : '1.0.0',
        platform,
        architecture,
        hookStatus:
          scenario === 'permission-required' || scenario === 'permission-recovers'
            ? 'permission_required'
            : 'ready',
        permissions: permissions(),
      });
      initialized = true;
    };
    if (scenario === 'slow-initialize') setTimeout(initialize, 150);
    else initialize();
    return;
  }
  if (method === 'activation.configure') {
    if (
      (scenario === 'permission-required' || scenario === 'permission-recovers') &&
      params.enabled === true
    )
      process.exit(27);
    if (scenario === 'reject-default-config' && !configured && params.enabled === false) {
      process.exit(26);
    }
    const expectedEnabled = scenario === 'expect-enabled';
    const expectedTrigger = expectedEnabled ? 'Q' : null;
    if (
      scenario === 'expect-enabled' &&
      !configured &&
      (params.enabled !== expectedEnabled ||
        !Array.isArray(params.bindings) ||
        (expectedTrigger !== null &&
          params.bindings[0]?.shortcut?.keys?.at(-1) !== expectedTrigger))
    )
      process.exit(25);
    configured = true;
    respond(id, params);
    return;
  }
  if (method === 'session.set_capture') respond(id, params);
  else if (method === 'paste.inject') {
    if (scenario === 'paste-hang') return;
    if (scenario !== 'paste-before-dispatch') notify('paste.committed', { requestId: id });
    if (scenario === 'paste-commit-hang') return;
    if (scenario === 'paste-delay') setTimeout(() => respond(id, { submitted: true }), 50);
    else if (scenario === 'paste-late-false')
      setTimeout(() => respond(id, { submitted: false, reason: 'os_rejected' }), 50);
    else if (scenario === 'paste-late-reject')
      setTimeout(() => respondError(id, -32003, 'Native operation unavailable'), 50);
    else if (scenario === 'paste-before-dispatch')
      setTimeout(() => respond(id, { submitted: false, reason: 'unavailable' }), 50);
    else respond(id, { submitted: true });
  } else if (method === 'front_app.get') {
    respond(id, { processName: 'fixture-app', windowTitle: 'Fixture target', windowBounds: null });
  } else if (method === 'permissions.get') respond(id, permissions());
  else if (method === 'ping') {
    respond(id, {
      ok: true,
      hookStatus:
        scenario === 'permission-required' || scenario === 'permission-recovers'
          ? 'permission_required'
          : 'ready',
    });
    if (scenario === 'notify') {
      notify('activation.event', {
        phase: 'down',
        profileId: 'general',
        shortcut: legacyShortcut('Z', false),
      });
    }
  } else if (method === 'shutdown') {
    const finish = () => {
      respond(id, {});
      process.stdout.write('', () => process.exit(0));
    };
    if (scenario === 'slow-shutdown') setTimeout(finish, 150);
    else finish();
  } else respondError(id, -32601, 'Method not found');
}

function legacyShortcut(key, shift) {
  return {
    modifiers: { ctrl: false, alt: true, shift, meta: false },
    keys: [key],
  };
}

function permissions() {
  return {
    accessibility:
      scenario === 'permission-required' || (scenario === 'permission-recovers' && !initialized)
        ? 'denied'
        : 'not_applicable',
    inputMonitoring: 'not_applicable',
    eventPost: 'not_applicable',
  };
}

function respond(id, result) {
  process.stdout.write(frame(Buffer.from(JSON.stringify({ jsonrpc: '2.0', id, result }))));
}

function notify(method, params) {
  process.stdout.write(frame(Buffer.from(JSON.stringify({ jsonrpc: '2.0', method, params }))));
}

function respondError(id, code, message) {
  process.stdout.write(
    frame(Buffer.from(JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } }))),
  );
}

function frame(payload) {
  const output = Buffer.allocUnsafe(payload.length + 4);
  output.writeUInt32BE(payload.length, 0);
  payload.copy(output, 4);
  return output;
}
