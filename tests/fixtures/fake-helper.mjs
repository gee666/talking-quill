const scenario = process.argv[2] ?? 'normal';
const platform = process.platform === 'win32' ? 'windows' : 'macos';
const architecture = process.arch === 'x64' ? 'x86_64' : 'aarch64';
let pending = Buffer.alloc(0);
let configured = false;
let startupConfigurationStage = 0;
let initialized = false;
let healthChecks = 0;
let activationNotificationSent = false;

if (scenario === 'forged-collision-diagnostic') {
  process.stderr.write('talking-quill-helper: forged OWNER_SINGLETON_COLLISION\n', () =>
    process.exit(1),
  );
}
if (scenario === 'protected-gateway-singleton-exit') process.exit(74);
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
    if (scenario === 'protocol-v7') {
      respondError(id, -32001, 'Incompatible protocol version');
      return;
    }
    if (params.protocolVersion !== 10) process.exit(28);
    const initialize = () => {
      respond(id, {
        protocolVersion: 10,
        helperVersion: scenario === 'mismatch' ? '9.9.9' : '1.0.0',
        platform,
        architecture,
        hookStatus:
          scenario === 'permission-required' || scenario === 'permission-recovers'
            ? 'permission_required'
            : 'installed_unobserved',
        permissions: permissions(),
        keyboardCapture: {
          activationAvailable: !scenario.startsWith('owner-state-'),
          sessionKeyCaptureAvailable: !scenario.startsWith('owner-state-'),
          runtimeRollbackActive: false,
          buildDisabled: false,
        },
        keyboardOwner: ownerSnapshot(),
      });
      initialized = true;
    };
    if (scenario === 'slow-initialize') setTimeout(initialize, 150);
    else initialize();
    return;
  }
  if (method === 'activation.configure') {
    const ownerRpcCode = {
      'owner-rpc-auth': -32005,
      'owner-rpc-incompatible': -32006,
      'owner-rpc-busy': -32007,
      'owner-rpc-draining': -32008,
      'owner-rpc-rollback': -32009,
      'owner-rpc-security': -32010,
      'owner-rpc-indeterminate': -32011,
    }[scenario];
    if (ownerRpcCode !== undefined) {
      respondError(id, ownerRpcCode, 'Keyboard owner unavailable');
      return;
    }
    if (
      (scenario === 'permission-required' || scenario === 'permission-recovers') &&
      params.enabled === true
    )
      process.exit(27);
    if (scenario === 'reject-default-config' && !configured && params.enabled === false) {
      process.exit(26);
    }
    if (scenario === 'expect-enabled' && startupConfigurationStage < 2) {
      const expectedEnabled = startupConfigurationStage === 1;
      if (
        params.enabled !== expectedEnabled ||
        !Array.isArray(params.bindings) ||
        params.bindings[0]?.shortcut?.keys?.at(-1) !== 'Q'
      )
        process.exit(25);
      startupConfigurationStage += 1;
    }
    configured = true;
    respond(id, params);
    return;
  }
  if (method === 'session.set_capture') respond(id, params);
  else if (method === 'paste.inject') {
    if (
      !Number.isSafeInteger(params.activationGeneration) ||
      params.activationGeneration < 1 ||
      !('targetToken' in params) ||
      (params.targetToken !== null &&
        (typeof params.targetToken !== 'string' ||
          params.targetToken.length === 0 ||
          Buffer.byteLength(params.targetToken, 'utf8') > 64)) ||
      typeof params.expectedClipboardSha256 !== 'string' ||
      !/^[0-9a-f]{64}$/.test(params.expectedClipboardSha256)
    )
      process.exit(29);
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
  else if (method === 'runtime.observability') {
    respond(id, runtimeObservability());
  } else if (method === 'ping') {
    respond(id, {
      ok: true,
      hookStatus:
        scenario === 'permission-required' ||
        (scenario === 'permission-recovers' && healthChecks === 0)
          ? 'permission_required'
          : 'installed_unobserved',
      keyboardOwner: ownerSnapshot(),
    });
    healthChecks += 1;
    if (scenario === 'notify' && !activationNotificationSent) {
      activationNotificationSent = true;
      notify('activation.event', {
        phase: 'down',
        profileId: 'general',
        shortcut: legacyShortcut('Z', false),
        activationGeneration: 1,
        targetToken: null,
      });
    }
  } else if (method === 'owner.prepare_maintenance') {
    respond(id, { maintenanceReady: true, ownerHandoff: '11'.repeat(32) });
  } else if (method === 'shutdown') {
    const finish = () => {
      if (scenario.startsWith('terminal-observability')) {
        const observability = runtimeObservability();
        process.stderr.write(
          `${JSON.stringify({
            event: 'helper.runtime.terminal',
            outcome: 'shutdown',
            observability: { ...observability, targetToken: 'must-not-cross-sink' },
          })}\n`,
        );
        observability.transactions.cancelled = 1;
        observability.transactions.cancellationReasons.shutdown = 1;
        observability.paste.nativeWaitDurationMsTotal = 75;
        observability.paste.nativeWaitDurationMsMax = 75;
        const terminalRecord = `${JSON.stringify({
          event: 'helper.runtime.terminal',
          outcome: scenario === 'terminal-observability-mismatched' ? 'failure' : 'shutdown',
          observability,
        })}\n`;
        process.stderr.write(terminalRecord);
        if (scenario === 'terminal-observability-duplicate') {
          process.stderr.write(terminalRecord);
        }
      }
      respond(id, {
        ownerDisposition: scenario === 'shutdown-draining' ? 'draining' : 'neutral',
      });
      process.stdout.write('', () => process.exit(0));
    };
    if (scenario === 'slow-shutdown') setTimeout(finish, 150);
    else finish();
  } else respondError(id, -32601, 'Method not found');
}

function runtimeObservability() {
  const zeroEffects = { attempted: 0, succeeded: 0, partial: 0, failed: 0 };
  return {
    keyboardOwner: ownerSnapshot(),
    owner: {
      starts: 1,
      cleanExits: 0,
      abnormalExits: 0,
      singletonCollisions: 0,
      authAttempts: 1,
      authFailures: { crossUser: 0, wrongSession: 0, codeIdentity: 0, mac: 0, protocol: 0 },
      leaseAcquired: 1,
      leaseRenewed: 1,
      leaseExpired: 0,
      leaseDisconnected: 0,
      leaseReleasedNeutral: 0,
      leaseReleasedDraining: 0,
      drainDurationMsTotal: 0,
      drainDurationMsMax: 0,
      maintenancePostponed: 0,
      handoffSucceeded: 0,
      handoffFailed: 0,
      degraded: 0,
      hookRecoveries: 0,
    },
    registeredInput: {
      hookInstalled: 1,
      pumpAlive: 1,
      hcActionCallbacks: 0,
      physicalCallbacks: 0,
      physicalCallbacksFiltered: 0,
      registeredCandidateCallbacks: 0,
      registeredMatchCallbacks: 0,
      registeredReleaseCallbacks: 0,
      callbackChannelAccepted: 0,
      callbackChannelRejected: 0,
      adapterDequeued: 0,
      ownerAdmitted: 0,
      ownerFlushed: 0,
      ownerRejected: 0,
      gatewayReceived: 0,
      v10NotificationAccepted: 0,
      electronReceived: 0,
      observationAccepted: 0,
    },
    keyboardCapture: {
      runtimeRollbackActive: false,
      developmentDisabled: false,
      activationEnableRequestsBlocked: 0,
      sessionCaptureRequestsBlocked: 0,
      shutdownOwnershipDeadlines: 0,
      terminalDisablements: 0,
    },
    transactions: {
      started: 0,
      committed: 0,
      replayed: 0,
      cancelled: 0,
      journalHighWater: 0,
      cancellationReasons: {
        invalidContinuation: 0,
        modifierChanged: 0,
        altGr: 0,
        journalOverflow: 0,
        configurationReplaced: 0,
        revisionMismatch: 0,
        gateClosed: 0,
        shutdown: 0,
        helperDisconnected: 0,
        secureDesktop: 0,
        timeout: 0,
        activationDeliveryFailed: 0,
        neutralizationFailed: 0,
        replayFailed: 0,
        effectProtocolViolation: 0,
        physicalStateMismatch: 0,
        targetChanged: 0,
      },
    },
    replay: zeroEffects,
    dummy: zeroEffects,
    paste: {
      attempted: 0,
      submitted: 0,
      targetValidationFallback: 0,
      nativeWaitDurationMsTotal: 0,
      nativeWaitDurationMsMax: 0,
      modifierTimeouts: 0,
      failures: {
        permissionDenied: 0,
        secureInput: 0,
        conflictingModifiers: 0,
        osRejected: 0,
        unavailable: 0,
        indeterminate: 0,
      },
    },
  };
}

function ownerSnapshot() {
  const ownerState = scenario.startsWith('owner-state-')
    ? scenario.slice('owner-state-'.length).replaceAll('-', '_')
    : 'leased_disabled';
  const authenticated = !['unavailable', 'degraded'].includes(ownerState);
  return {
    model: 'out_of_process',
    protocolVersion: 1,
    state: ownerState,
    instanceId: !authenticated
      ? ''
      : scenario === 'owner-instance-mismatch' && initialized
        ? 'owner-instance-2'
        : 'owner-instance-1',
    buildId: authenticated ? 'owner-build-1' : '',
    leaseEpoch: !authenticated ? null : scenario === 'owner-epoch-mismatch' && initialized ? 2 : 1,
    authenticated,
  };
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
      scenario === 'permission-required' ||
      (scenario === 'permission-recovers' && healthChecks === 0)
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
