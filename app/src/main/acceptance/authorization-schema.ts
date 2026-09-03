import { z } from 'zod';

const Hex32Schema = z.string().regex(/^[0-9a-f]{64}$/u);
const Base64UrlSchema = z.string().regex(/^[A-Za-z0-9_-]+$/u);
export const InstalledReadinessPipeSchema = z
  .string()
  .regex(/^\\\\\.\\pipe\\TalkingQuill\.InstalledReadiness\.[0-9a-f]{32}$/u);
export const InstalledAutomationArmedPipeSchema = z
  .string()
  .regex(/^\\\\\.\\pipe\\TalkingQuill\.AutomationArmed\.[0-9a-f]{32}$/u);
export const InstalledAutomationCaseSchema = z
  .string()
  .refine(
    (value) =>
      ['general', 'prompt', 'replay', 'lifecycle'].includes(value) ||
      /^lifecycle-waiter-[bc]$/u.test(value),
    'Invalid installed automation case',
  );

export const AcceptanceBuildManifestPayloadSchema = z
  .object({
    version: z.literal(1),
    purpose: z.literal('talking-quill/installed-acceptance-build'),
    sourceRevision: z.string().regex(/^[0-9a-f]{12}$/u),
    buildId: Hex32Schema,
    architecture: z.enum(['x64', 'arm64']),
    packageVersion: z.string().regex(/^\d+\.\d+\.\d+$/u),
    releaseBuildDigest: Hex32Schema,
    packageLayoutDigest: Hex32Schema,
    ownerManifestSha256: Hex32Schema,
    electronSha256: Hex32Schema,
    appAsarSha256: Hex32Schema,
    gatewaySha256: Hex32Schema,
    ownerSha256: Hex32Schema,
    requestPublicKeySpkiBase64url: Base64UrlSchema.max(256),
    validationPublicKeySpkiBase64url: Base64UrlSchema.max(256),
    faultValidationPolicy: z
      .object({
        schemaVersion: z.literal(1),
        phases: z.tuple([
          z.literal('staged'),
          z.literal('prepared'),
          z.literal('predecessorMoved'),
          z.literal('publishing'),
          z.literal('publishedBeforePersist'),
          z.literal('published'),
          z.literal('registered'),
          z.literal('committed'),
          z.literal('legacyRetiring'),
          z.literal('legacyRetired'),
        ]),
        validatorSha256: Hex32Schema,
      })
      .strict(),
    validFromMs: z.number().int().nonnegative(),
    validUntilMs: z.number().int().positive(),
  })
  .strict()
  .refine((value) => value.validUntilMs > value.validFromMs, {
    message: 'Acceptance build validity interval is invalid',
  });

export const AcceptanceBuildManifestSchema = z
  .object({
    payload: AcceptanceBuildManifestPayloadSchema,
    signatureBase64url: Base64UrlSchema.length(86),
  })
  .strict();

export const AcceptanceCommandSchema = z.enum([
  'normal-readiness',
  'endpoint-peer',
  'heartbeat-120s',
  'gateway-reconnect-arm',
  'lease-expiry-arm',
  'electron-crash-arm',
  'login-marker',
  'diagnostics-disabled-failure',
  'manual-physical-observation',
  'supplemental-synthetic-observation',
]);

export const AcceptanceRunWindowSchema = z
  .object({
    notBeforeMs: z.number().int().nonnegative(),
    expiresAtMs: z.number().int().positive(),
    maxTotalRunMs: z
      .number()
      .int()
      .positive()
      .max(80 * 60 * 1_000),
  })
  .strict()
  .superRefine((value, context) => {
    if (
      value.expiresAtMs <= value.notBeforeMs ||
      value.expiresAtMs - value.notBeforeMs < value.maxTotalRunMs
    ) {
      context.addIssue({ code: 'custom', message: 'Acceptance run window is invalid' });
    }
  });

export const AcceptanceRunRequestPayloadSchema = z
  .object({
    version: z.literal(1),
    purpose: z.literal('talking-quill/installed-acceptance-run'),
    command: AcceptanceCommandSchema,
    buildId: Hex32Schema,
    invocationId: z
      .string()
      .regex(/^[a-z0-9-]+$/u)
      .max(64),
    latestStartOffsetMs: z.number().int().nonnegative(),
    deadlineOffsetMs: z.number().int().positive(),
    runWindow: AcceptanceRunWindowSchema,
    requestNonce: Hex32Schema,
    issuedAtMs: z.number().int().nonnegative(),
    expiresAtMs: z.number().int().positive(),
    readinessPipe: InstalledReadinessPipeSchema,
    launchCorrelation: Hex32Schema,
    physicalObservation: z.boolean(),
    automationValidation: z.boolean(),
    automationArmedPipe: InstalledAutomationArmedPipeSchema.nullable(),
    automationCase: InstalledAutomationCaseSchema.nullable(),
    lifecycleUserData: z.string().min(1).max(1_024).nullable(),
    heartbeatDurationMs: z.union([z.literal(6_250), z.literal(120_000)]),
  })
  .strict()
  .superRefine((value, context) => {
    if (
      value.latestStartOffsetMs >= value.deadlineOffsetMs ||
      value.deadlineOffsetMs > value.runWindow.maxTotalRunMs
    ) {
      context.addIssue({ code: 'custom', message: 'Acceptance invocation schedule is invalid' });
    }
    if (value.expiresAtMs <= value.issuedAtMs) {
      context.addIssue({
        code: 'custom',
        message: 'Acceptance request validity interval is invalid',
      });
    }
    if (
      value.physicalObservation !== (value.command === 'manual-physical-observation') ||
      value.automationValidation !==
        [
          'gateway-reconnect-arm',
          'electron-crash-arm',
          'supplemental-synthetic-observation',
        ].includes(value.command)
    ) {
      context.addIssue({ code: 'custom', message: 'Acceptance mode does not match its command' });
    }
    if (value.heartbeatDurationMs !== (value.command === 'heartbeat-120s' ? 120_000 : 6_250)) {
      context.addIssue({ code: 'custom', message: 'Acceptance heartbeat duration is invalid' });
    }
    if (value.physicalObservation && value.automationValidation) {
      context.addIssue({
        code: 'custom',
        message: 'Acceptance observation modes are mutually exclusive',
      });
    }
    const armedChannelRequired =
      value.automationValidation ||
      value.command === 'login-marker' ||
      value.command === 'manual-physical-observation';
    if (armedChannelRequired !== (value.automationArmedPipe !== null)) {
      context.addIssue({
        code: 'custom',
        message: 'Acceptance armed pipe does not match its command',
      });
    }
    if (value.automationValidation !== (value.automationCase !== null)) {
      context.addIssue({ code: 'custom', message: 'Acceptance case does not match its mode' });
    }
  });

export const AcceptanceRunRequestSchema = z
  .object({
    payload: AcceptanceRunRequestPayloadSchema,
    signatureBase64url: Base64UrlSchema.length(86),
  })
  .strict();

export type AcceptanceBuildManifest = z.infer<typeof AcceptanceBuildManifestSchema>;
export type AcceptanceBuildManifestPayload = z.infer<typeof AcceptanceBuildManifestPayloadSchema>;
export type AcceptanceRunRequest = z.infer<typeof AcceptanceRunRequestSchema>;
export type AcceptanceRunRequestPayload = z.infer<typeof AcceptanceRunRequestPayloadSchema>;
