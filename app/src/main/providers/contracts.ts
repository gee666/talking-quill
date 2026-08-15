import type {
  Destination,
  ModelInfo,
  ProviderCompletionRequest,
  ProviderConfig,
  ProviderId,
  ProviderValidationResult,
  VisionCapability,
} from '../../shared/schemas/providers';

export type ProviderCredentialPolicy = 'none' | 'optional' | 'required';

export interface ProviderInvocationConfig {
  readonly config: ProviderConfig;
  readonly credential: string | null;
  /** Explicit user discovery bypasses provider-local catalog caches. */
  readonly refreshModels?: boolean;
}

export type PreparedCompletionCloseReason = string;

/** Opaque provider-owned capability for exactly one prepared completion. */
export interface PreparedProviderCompletion {
  complete(request: ProviderCompletionRequest, signal: AbortSignal): Promise<string>;
  /** Starts retirement. Implementations must make repeated calls harmless. */
  requestClose(reason: PreparedCompletionCloseReason): void;
  /** Settles only after every provider-owned resource has retired. */
  readonly closed: Promise<void>;
}

/** The application-facing lease has the same deliberately opaque surface. */
export type PreparedCompletionLease = PreparedProviderCompletion;

export interface SmartProvider {
  readonly id: ProviderId;
  readonly credentialPolicy: ProviderCredentialPolicy;
  credentialBinding(config: ProviderConfig): string;
  validate(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ProviderValidationResult>;
  listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]>;
  capabilities(config: ProviderConfig, modelId: string): VisionCapability;
  capabilityPreflight?(
    invocation: ProviderInvocationConfig,
    modelId: string,
    signal: AbortSignal,
  ): Promise<VisionCapability>;
  cleanTranscript(
    invocation: ProviderInvocationConfig,
    request: ProviderCompletionRequest,
    signal: AbortSignal,
  ): Promise<string>;
  readonly prepareCompletion?: (
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ) => Promise<PreparedProviderCompletion | null>;
  classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination>;
}

export interface CredentialResolver {
  getCredential(
    providerId: ProviderId,
    endpointBinding: string,
  ): string | null | Promise<string | null>;
}
