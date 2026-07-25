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
