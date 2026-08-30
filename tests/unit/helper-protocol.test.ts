import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  HelperFrameDecoder,
  decodeHelperJson,
  encodeHelperFrame,
} from '../../app/src/main/helper/framing';
import {
  HELPER_MAX_FRAME_BYTES,
  HELPER_MAX_INSERTION_UTF8_BYTES,
  HelperAcceptanceEndpointObservabilitySchema,
  HelperAcceptancePauseLeaseRenewalResultSchema,
  HelperKeyboardOwnerSnapshotSchema,
  HelperNotificationSchema,
  HelperRequestIdSchema,
  HelperRuntimeObservabilitySchema,
  HelperRpcResponseSchema,
  HelperTerminalObservabilityRecordSchema,
  helperParamsSchemas,
  helperResultSchemas,
} from '../../app/src/shared/helper/protocol';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
} from '../../app/src/shared/schemas/dictation-profiles';
import { shortcutFromLegacyActivation } from '../../app/src/shared/schemas/shortcut';
import { TRANSCRIPT_MAX_UTF8_BYTES } from '../../app/src/shared/schemas/transcription';

const binding = (profileId: string, shortcut: unknown) => ({ profileId, shortcut });
const customProfileId = (index: number) =>
  `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`;
const activationContext = { activationGeneration: 1, targetToken: null } as const;
const pasteParams = {
  ...activationContext,
  expectedClipboardSha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
} as const;
const ownerObservability = {
  starts: 1,
  cleanExits: 0,
  abnormalExits: 0,
  singletonCollisions: 0,
  authAttempts: 1,
  authFailures: { crossUser: 0, wrongSession: 0, codeIdentity: 0, mac: 0, protocol: 0 },
  leaseAcquired: 1,
  leaseRenewed: 7,
  leaseExpired: 2,
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
} as const;
const ownerSnapshot = {
  model: 'out_of_process',
  protocolVersion: 1,
  state: 'leased_disabled',
  instanceId: 'owner-instance-1',
  buildId: 'owner-build-1',
  leaseEpoch: 1,
  authenticated: true,
} as const;

describe('native helper framing', () => {
  it('shares the application transcript UTF-8 bound with native insertion', () => {
    expect(HELPER_MAX_INSERTION_UTF8_BYTES).toBe(TRANSCRIPT_MAX_UTF8_BYTES);
    expect(HELPER_MAX_INSERTION_UTF8_BYTES).toBe(1_000_000);
  });
  it('decodes partial and concatenated big-endian frames', () => {
    const first = encodeHelperFrame({ one: 1 });
    const second = encodeHelperFrame({ two: 2 });
    const decoder = new HelperFrameDecoder();
    expect(decoder.push(first.subarray(0, 2))).toEqual([]);
    const frames = decoder.push(Buffer.concat([first.subarray(2), second]));
    expect(frames.map(decodeHelperJson)).toEqual([{ one: 1 }, { two: 2 }]);
    expect(() => decoder.finish()).not.toThrow();
  });

  it('decodes a maximum-size frame fragmented one byte at a time', () => {
    const payload = Buffer.alloc(HELPER_MAX_FRAME_BYTES, 0x61);
    const framed = Buffer.allocUnsafe(payload.length + 4);
    framed.writeUInt32BE(payload.length, 0);
    payload.copy(framed, 4);
    const decoder = new HelperFrameDecoder();
    const frames: Buffer[] = [];

    for (const byte of framed) frames.push(...decoder.push(Buffer.of(byte)));

    expect(frames).toEqual([payload]);
    expect(() => decoder.finish()).not.toThrow();
  });

  it('rejects zero, oversized, truncated, and invalid UTF-8 frames', () => {
    const zero = Buffer.alloc(4);
    expect(() => new HelperFrameDecoder().push(zero)).toThrow('Invalid helper frame length');
    const oversized = Buffer.alloc(4);
    oversized.writeUInt32BE(HELPER_MAX_FRAME_BYTES + 1);
    expect(() => new HelperFrameDecoder().push(oversized)).toThrow('Invalid helper frame length');

    const truncated = new HelperFrameDecoder();
    expect(truncated.push(encodeHelperFrame({ ok: true }).subarray(0, 7))).toEqual([]);
    expect(() => truncated.finish()).toThrow('truncated frame');
    expect(() => decodeHelperJson(Buffer.from([0xff]))).toThrow('invalid UTF-8 JSON');
    expect(() => encodeHelperFrame('x'.repeat(HELPER_MAX_FRAME_BYTES))).toThrow('Outbound helper');
  });
});

describe('native helper JSON-RPC schemas', () => {
  it('accepts only redacted authenticated V2 Windows endpoint facts', () => {
    const observation = {
      endpointVersion: 2,
      peerAuthenticated: true,
      releaseBuildDigest: '22'.repeat(32),
      manifestSha256: '33'.repeat(32),
      gateway: {
        processId: 41,
        creationMarker: '133700000000000001',
        integrityRid: 8192,
        sessionId: 3,
        userSidHash: '11'.repeat(32),
      },
      owner: {
        processId: 42,
        creationMarker: '133700000000000002',
        integrityRid: 8192,
        sessionId: 3,
        userSidHash: '11'.repeat(32),
      },
    } as const;
    expect(HelperAcceptanceEndpointObservabilitySchema.parse(observation)).toEqual(observation);
    expect(
      HelperAcceptanceEndpointObservabilitySchema.safeParse({
        ...observation,
        userSid: 'S-1-5-21-secret',
      }).success,
    ).toBe(false);
    expect(
      HelperAcceptanceEndpointObservabilitySchema.safeParse({
        ...observation,
        peerAuthenticated: false,
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['acceptance.endpoint_observability'].safeParse({ extra: true }).success,
    ).toBe(false);
  });

  it('requires the fixed 6500ms lease pause and exactly one expiry', () => {
    const result = {
      pauseDurationMs: 6_500,
      beforeTimestampMs: 1_700_000_000_000,
      afterTimestampMs: 1_700_000_006_500,
      before: ownerObservability,
      after: { ...ownerObservability, leaseExpired: 3 },
    } as const;
    expect(HelperAcceptancePauseLeaseRenewalResultSchema.safeParse(result).success).toBe(true);
    expect(
      HelperAcceptancePauseLeaseRenewalResultSchema.safeParse({
        ...result,
        pauseDurationMs: 100,
      }).success,
    ).toBe(false);
    expect(
      HelperAcceptancePauseLeaseRenewalResultSchema.safeParse({
        ...result,
        after: { ...result.after, leaseExpired: 4 },
      }).success,
    ).toBe(false);
    expect(
      HelperAcceptancePauseLeaseRenewalResultSchema.safeParse({
        ...result,
        after: { ...result.after, leaseRenewed: 8 },
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['acceptance.pause_lease_renewal'].safeParse({ durationMs: 6_500 })
        .success,
    ).toBe(false);
  });

  it('requires protocol v10 and rejects v9 without changing shortcut grammar', () => {
    expect(helperParamsSchemas.initialize.safeParse({ protocolVersion: 10 }).success).toBe(true);
    expect(helperParamsSchemas.initialize.safeParse({ protocolVersion: 9 }).success).toBe(false);
    const initialized = {
      protocolVersion: 10,
      helperVersion: '1.0.0',
      platform: 'windows',
      architecture: 'x86_64',
      hookStatus: 'installed_unobserved',
      permissions: {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      },
      keyboardCapture: {
        activationAvailable: true,
        sessionKeyCaptureAvailable: true,
        runtimeRollbackActive: false,
        buildDisabled: false,
      },
      keyboardOwner: ownerSnapshot,
    };
    expect(helperResultSchemas.initialize.safeParse(initialized).success).toBe(true);
    expect(
      helperResultSchemas.initialize.safeParse({ ...initialized, defaultActivationKey: 'Z' })
        .success,
    ).toBe(false);
    expect(
      helperResultSchemas.initialize.safeParse({
        ...initialized,
        keyboardOwner: { ...ownerSnapshot, state: 'leased_enabled' },
      }).success,
    ).toBe(false);
  });

  it('rejects contradictory or owner-ineligible keyboard capture capabilities', () => {
    const initialized = {
      protocolVersion: 10,
      helperVersion: '1.0.0',
      platform: 'windows',
      architecture: 'x86_64',
      hookStatus: 'installed_unobserved',
      permissions: {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      },
      keyboardOwner: ownerSnapshot,
    } as const;

    for (const [buildDisabled, runtimeRollbackActive] of [
      [false, false],
      [false, true],
      [true, false],
      [true, true],
    ] as const) {
      const expectedAvailable = !buildDisabled && !runtimeRollbackActive;
      const keyboardCapture = {
        activationAvailable: expectedAvailable,
        sessionKeyCaptureAvailable: expectedAvailable,
        runtimeRollbackActive,
        buildDisabled,
      };
      expect(
        helperResultSchemas.initialize.safeParse({ ...initialized, keyboardCapture }).success,
      ).toBe(true);
      expect(
        helperResultSchemas.initialize.safeParse({
          ...initialized,
          keyboardCapture: {
            ...keyboardCapture,
            activationAvailable: !expectedAvailable,
          },
        }).success,
      ).toBe(false);
      expect(
        helperResultSchemas.initialize.safeParse({
          ...initialized,
          keyboardCapture: {
            ...keyboardCapture,
            sessionKeyCaptureAvailable: !expectedAvailable,
          },
        }).success,
      ).toBe(false);
    }
  });

  it('documents the mandatory protocol-v10 owner boundary with no executable v8 fallback', () => {
    const documentation = readFileSync('helper/src/protocol/mod.rs', 'utf8');
    expect(documentation).toContain('protocol (strict JSON-RPC 2.0, version 10)');
    expect(documentation).toContain('keyboard-owner snapshot');
    expect(documentation).toContain('shutdown reports');
    expect(documentation).toContain('owner.prepare_maintenance');
    expect(documentation).not.toContain('PROTOCOL_VERSION: u16 = 8');
    for (const path of [
      'app/src/shared/helper/protocol.ts',
      'app/src/main/helper/helper-client.ts',
      'tests/fixtures/fake-helper.mjs',
      'tests/native/helper-harness.mjs',
    ]) {
      const source = readFileSync(path, 'utf8');
      expect(source, path).not.toMatch(/protocolVersion\s*[:=]\s*8/u);
      expect(source, path).not.toContain('HELPER_PROTOCOL_VERSION = 8');
    }
  });

  it('requires strict authenticated owner snapshots and permits fail-closed unavailable snapshots', () => {
    expect(HelperKeyboardOwnerSnapshotSchema.safeParse(ownerSnapshot).success).toBe(true);
    expect(
      HelperKeyboardOwnerSnapshotSchema.safeParse({
        ...ownerSnapshot,
        state: 'unavailable',
        instanceId: '',
        buildId: '',
        leaseEpoch: null,
        authenticated: false,
      }).success,
    ).toBe(true);
    for (const invalid of [
      { ...ownerSnapshot, model: 'in_process' },
      { ...ownerSnapshot, protocolVersion: 2 },
      { ...ownerSnapshot, instanceId: '' },
      { ...ownerSnapshot, leaseEpoch: null },
      { ...ownerSnapshot, authenticated: false },
      { ...ownerSnapshot, authenticated: false, leaseEpoch: null },
      { ...ownerSnapshot, endpoint: 'secret' },
    ]) {
      expect(HelperKeyboardOwnerSnapshotSchema.safeParse(invalid).success).toBe(false);
    }
  });

  it('contains only the fixed command allowlist with strict params', () => {
    expect(Object.keys(helperParamsSchemas)).toEqual([
      'initialize',
      'activation.configure',
      'session.set_capture',
      'paste.inject',
      'front_app.get',
      'permissions.get',
      'runtime.observability',
      'acceptance.endpoint_observability',
      'acceptance.pause_lease_renewal',
      'ping',
      'diagnostic.ack',
      'owner.prepare_maintenance',
      'shutdown',
    ]);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [binding('general', shortcutFromLegacyActivation('Z', false))],
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [
          binding('prompt', {
            modifiers: { ctrl: true, alt: false, shift: true, meta: false },
            keys: ['Q', 'P'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: true, shift: false, meta: false },
            keys: ['F1'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({ enabled: true, key: 'Z' }).success,
    ).toBe(false);
    expect(helperParamsSchemas['paste.inject'].safeParse(pasteParams).success).toBe(true);
    expect(helperParamsSchemas['paste.inject'].safeParse(activationContext).success).toBe(false);
    expect(
      helperParamsSchemas['paste.inject'].safeParse({
        ...pasteParams,
        expectedClipboardSha256: pasteParams.expectedClipboardSha256.toUpperCase(),
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['paste.inject'].safeParse({
        ...pasteParams,
        expectedClipboardSha256: 'e3b0',
      }).success,
    ).toBe(false);
    expect(helperParamsSchemas['paste.inject'].safeParse({}).success).toBe(false);
    const diagnosticAck = {
      journalId: '01'.repeat(32),
      journalNonce: '23'.repeat(32),
      dimensions: {
        category: 'disconnected',
        operation: 'lease.renew',
        correlationStatus: 'pending',
        healthRefresh: 'not_attempted',
        transportStatus: 'eof',
        ownerProcessState: 'running',
      },
      count: '40',
    } as const;
    expect(helperParamsSchemas['diagnostic.ack'].safeParse(diagnosticAck).success).toBe(true);
    for (const sentinel of ['0'.repeat(64), 'f'.repeat(64)]) {
      expect(
        helperParamsSchemas['diagnostic.ack'].safeParse({
          ...diagnosticAck,
          journalId: sentinel,
        }).success,
      ).toBe(false);
      expect(
        helperParamsSchemas['diagnostic.ack'].safeParse({
          ...diagnosticAck,
          journalNonce: sentinel,
        }).success,
      ).toBe(false);
    }
    expect(
      helperParamsSchemas['diagnostic.ack'].safeParse({
        ...diagnosticAck,
        journalId: '01'.repeat(31),
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['diagnostic.ack'].safeParse({ ...diagnosticAck, token: 'secret' })
        .success,
    ).toBe(false);
    expect(
      helperParamsSchemas['diagnostic.ack'].safeParse({ ...diagnosticAck, count: '-1' }).success,
    ).toBe(false);
    const update = {
      operation: 'update',
      transactionId: 'transaction-1',
      sourceBuildId: 'source-build',
      targetBuildId: 'target-build',
      targetOwnerSha256: 'a'.repeat(64),
    } as const;
    expect(helperParamsSchemas['owner.prepare_maintenance'].safeParse(update).success).toBe(true);
    expect(
      helperParamsSchemas['owner.prepare_maintenance'].safeParse({
        operation: 'uninstall',
        transactionId: 'transaction-1',
        sourceBuildId: 'source-build',
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['owner.prepare_maintenance'].safeParse({
        ...update,
        operation: 'uninstall',
      }).success,
    ).toBe(false);
    expect(helperResultSchemas.shutdown.safeParse({ ownerDisposition: 'draining' }).success).toBe(
      true,
    );
    expect(helperResultSchemas.shutdown.safeParse({}).success).toBe(false);
    expect(
      helperParamsSchemas['paste.inject'].safeParse({
        ...pasteParams,
        key: 'A',
        shell: 'whoami',
      }).success,
    ).toBe(false);
    const capture = helperParamsSchemas['session.set_capture'];
    for (const mode of ['off', 'recording', 'cancel-only']) {
      expect(capture.safeParse({ mode }).success).toBe(true);
    }
    for (const value of [
      { active: true },
      { mode: 'cancel_only' },
      { mode: true },
      { mode: 'off', extra: true },
    ]) {
      expect(capture.safeParse(value).success).toBe(false);
    }
  });

  it('accepts only aggregate runtime observability without key or target fields', () => {
    const counters = { attempted: 2, succeeded: 1, partial: 0, failed: 1 };
    const result = {
      keyboardOwner: ownerSnapshot,
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
        hcActionCallbacks: 4,
        physicalCallbacks: 4,
        physicalCallbacksFiltered: 0,
        registeredCandidateCallbacks: 2,
        registeredMatchCallbacks: 1,
        registeredReleaseCallbacks: 1,
        callbackChannelAccepted: 1,
        callbackChannelRejected: 0,
        adapterDequeued: 1,
        ownerAdmitted: 1,
        ownerFlushed: 1,
        ownerRejected: 0,
        gatewayReceived: 1,
        v10NotificationAccepted: 1,
        electronReceived: 1,
        observationAccepted: 0,
      },
      keyboardCapture: {
        runtimeRollbackActive: true,
        developmentDisabled: false,
        activationEnableRequestsBlocked: 1,
        sessionCaptureRequestsBlocked: 1,
        shutdownOwnershipDeadlines: 0,
        terminalDisablements: 0,
      },
      transactions: {
        started: 2,
        committed: 1,
        replayed: 1,
        cancelled: 1,
        journalHighWater: 3,
        cancellationReasons: {
          invalidContinuation: 1,
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
      replay: counters,
      dummy: counters,
      paste: {
        attempted: 2,
        submitted: 1,
        targetValidationFallback: 1,
        nativeWaitDurationMsTotal: 120,
        nativeWaitDurationMsMax: 80,
        modifierTimeouts: 1,
        failures: {
          permissionDenied: 0,
          secureInput: 0,
          conflictingModifiers: 0,
          osRejected: 0,
          unavailable: 1,
          indeterminate: 0,
        },
      },
    };
    expect(HelperRuntimeObservabilitySchema.safeParse(result).success).toBe(true);
    expect(
      HelperTerminalObservabilityRecordSchema.safeParse({
        event: 'helper.runtime.terminal',
        outcome: 'shutdown',
        observability: result,
      }).success,
    ).toBe(true);
    expect(
      HelperTerminalObservabilityRecordSchema.safeParse({
        event: 'helper.runtime.terminal',
        outcome: 'shutdown',
        observability: { ...result, targetToken: 'secret' },
      }).success,
    ).toBe(false);
    expect(
      HelperRuntimeObservabilitySchema.safeParse({ ...result, targetToken: 'secret' }).success,
    ).toBe(false);
    expect(
      HelperRuntimeObservabilitySchema.safeParse({
        ...result,
        registeredInput: { ...result.registeredInput, rawKey: 'X' },
      }).success,
    ).toBe(false);
    expect(
      HelperRuntimeObservabilitySchema.safeParse({
        ...result,
        transactions: { ...result.transactions, shortcut: ['X', 'P'] },
      }).success,
    ).toBe(false);
    expect(
      HelperRuntimeObservabilitySchema.safeParse({
        ...result,
        paste: { ...result.paste, attempted: Number.MAX_SAFE_INTEGER + 1 },
      }).success,
    ).toBe(false);
    expect(
      HelperRuntimeObservabilitySchema.safeParse({
        ...result,
        paste: {
          ...result.paste,
          nativeWaitDurationMsTotal: 2,
          nativeWaitDurationMsMax: 3,
        },
      }).success,
    ).toBe(false);
  });

  it('allows legacy/custom shared prefixes while rejecting exact duplicates and current wrong-owner reservations', () => {
    const schema = helperParamsSchemas['activation.configure'];
    const altA = shortcutFromLegacyActivation('A', false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', DEFAULT_GENERAL_PROFILE.shortcut),
          binding('prompt', DEFAULT_PROMPT_PROFILE.shortcut),
          binding('prompt-to-english', DEFAULT_PROMPT_TO_ENGLISH_PROFILE.shortcut),
          binding('markdown', DEFAULT_MARKDOWN_PROFILE.shortcut),
          binding('translate-to-english', DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: true, shift: false, meta: false },
            keys: ['X', 'Q'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', DEFAULT_PROMPT_PROFILE.shortcut)],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [] }).success).toBe(false);
    expect(schema.safeParse({ enabled: false, bindings: [] }).success).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: false, shift: false, meta: false },
            keys: ['A'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: false, shift: true, meta: false },
            keys: ['A'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', altA), binding('prompt', altA)],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', altA), binding('prompt', { ...altA, keys: ['A', 'B'] })],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', altA),
          binding('general', shortcutFromLegacyActivation('B', false)),
        ],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [binding('custom', altA)] }).success).toBe(
      false,
    );
    for (const profileId of [
      '00000000-0000-0000-0000-000000000000',
      'ffffffff-ffff-ffff-ffff-ffffffffffff',
    ]) {
      expect(
        schema.safeParse({ enabled: true, bindings: [binding(profileId, altA)] }).success,
      ).toBe(true);
    }
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF', altA)],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [altA] }).success).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', altA),
          binding('prompt', {
            ...altA,
            modifiers: { ...altA.modifiers, shift: true },
            keys: ['A', 'B'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', { ...altA, keys: ['A', 'A'] })],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: Array.from({ length: 13 }, (_, index) =>
          binding(customProfileId(index), {
            ...altA,
            keys: [String.fromCharCode(65 + index)],
          }),
        ),
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: Array.from({ length: 14 }, (_, index) =>
          binding(customProfileId(index), {
            ...altA,
            keys: [String.fromCharCode(65 + index)],
          }),
        ),
      }).success,
    ).toBe(false);
  });

  it('preserves the complete bounded activation context in v10', () => {
    const base = {
      jsonrpc: '2.0',
      method: 'activation.event',
      params: {
        phase: 'down',
        profileId: 'general',
        shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
      },
    } as const;
    for (const context of [
      { activationGeneration: 1, targetToken: null },
      { activationGeneration: Number.MAX_SAFE_INTEGER, targetToken: 'a'.repeat(64) },
      { activationGeneration: 2, targetToken: 'é'.repeat(32) },
    ]) {
      expect(
        HelperNotificationSchema.safeParse({
          ...base,
          params: { ...base.params, ...context },
        }).success,
      ).toBe(true);
    }
    for (const context of [
      {},
      { activationGeneration: 1 },
      { targetToken: null },
      { activationGeneration: 0, targetToken: null },
      { activationGeneration: -1, targetToken: null },
      { activationGeneration: 1.5, targetToken: null },
      { activationGeneration: Number.MAX_SAFE_INTEGER + 1, targetToken: null },
      { activationGeneration: 1, targetToken: '' },
      { activationGeneration: 1, targetToken: 'a'.repeat(65) },
      { activationGeneration: 1, targetToken: 'é'.repeat(33) },
      {
        activationGeneration: 1,
        targetToken: null,
        shortcut: { ...DEFAULT_GENERAL_PROFILE.shortcut, activationGeneration: 1 },
      },
    ]) {
      expect(
        HelperNotificationSchema.safeParse({
          ...base,
          params: { ...base.params, ...context },
        }).success,
      ).toBe(false);
    }
  });

  it('accepts bounded numeric and v2-compatible string request IDs', () => {
    for (const id of [0, Number.MAX_SAFE_INTEGER, 'request-id', 'é'.repeat(32)]) {
      expect(HelperRequestIdSchema.safeParse(id).success).toBe(true);
    }
    for (const id of [-1, Number.MAX_SAFE_INTEGER + 1, '', 'a'.repeat(65), 'é'.repeat(33)]) {
      expect(HelperRequestIdSchema.safeParse(id).success).toBe(false);
    }
  });

  it('strictly validates responses, method results, and notifications', () => {
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 1,
        result: { submitted: true },
      }).success,
    ).toBe(true);
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 'request-id',
        result: {},
      }).success,
    ).toBe(true);
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 1,
        result: {},
        extra: true,
      }).success,
    ).toBe(false);
    expect(helperResultSchemas['paste.inject'].safeParse({ submitted: false }).success).toBe(false);
    expect(
      helperResultSchemas['paste.inject'].safeParse({
        submitted: false,
        reason: 'secure_input',
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: 'translate-to-english',
          shortcut: DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut,
          ...activationContext,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'general',
          shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
          ...activationContext,
          heldMs: 0,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'general',
          shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
        },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'prompt',
          shortcut: DEFAULT_PROMPT_PROFILE.shortcut,
          activationGeneration: Number.MAX_SAFE_INTEGER,
          targetToken: 'é'.repeat(32),
          heldMs: 100,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: customProfileId(1),
          shortcut: shortcutFromLegacyActivation('A', true),
          activationGeneration: 2,
          targetToken: null,
          heldMs: 100,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'registered_input.observed',
        params: { generation: 9 },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'registered_input.observed',
        params: { generation: 0 },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: 'general',
          shortcut: { keys: ['Z'] },
          ...activationContext,
        },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          shortcut: shortcutFromLegacyActivation('Z', false),
          ...activationContext,
        },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: 'general',
          key: 'Z',
          shift: false,
          ...activationContext,
        },
      }).success,
    ).toBe(false);
    const inputDevicesChanged = {
      jsonrpc: '2.0',
      method: 'audio.input_devices_changed',
      params: {},
    } as const;
    expect(HelperNotificationSchema.safeParse(inputDevicesChanged).success).toBe(true);
    for (const malformed of [
      { ...inputDevicesChanged, params: null },
      { ...inputDevicesChanged, params: { endpointId: 'raw-core-audio-id' } },
      { jsonrpc: '2.0', method: inputDevicesChanged.method },
    ]) {
      expect(HelperNotificationSchema.safeParse(malformed).success).toBe(false);
    }
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'key.inject',
        params: { key: 'A' },
      }).success,
    ).toBe(false);
  });
});
