const scenario = process.argv[2] ?? 'normal';
const platform = process.platform === 'win32' ? 'windows' : 'macos';
const architecture = process.arch === 'x64' ? 'x86_64' : 'aarch64';
let pending = Buffer.alloc(0);
let configured = false;

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
    respond(id, {
      protocolVersion: 2,
      helperVersion: scenario === 'mismatch' ? '9.9.9' : '1.0.0',
      platform,
      architecture,
      defaultActivationKey: 'Z',
      hookStatus: 'ready',
      permissions: permissions(),
    });
    return;
  }
  if (method === 'activation.configure') {
    const expectedEnabled = scenario === 'expect-enabled';
    const expectedKey = expectedEnabled ? 'Q' : null;
    if (
      !configured &&
      (params.enabled !== expectedEnabled ||
        !Array.isArray(params.bindings) ||
        (expectedKey !== null && params.bindings[0]?.key !== expectedKey))
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
    respond(id, { ok: true, hookStatus: 'ready' });
    if (scenario === 'notify') {
      notify('activation.event', { phase: 'down', key: 'Z', shift: false });
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

function permissions() {
  return {
    accessibility: 'not_applicable',
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
