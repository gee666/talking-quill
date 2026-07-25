import { randomUUID } from 'node:crypto';
import {
  RUNNABLE_PROVIDER_IDS,
  RunnableProviderConfigSchema,
  RunnableProviderIdSchema,
  type RunnableProviderConfig,
  type RunnableProviderId,
} from '../../shared/schemas/providers';
import {
  ProviderCredentialBindingTokenSchema,
  ProviderCredentialStateSchema,
  serializeAwsCredentials,
  type ProviderCredentialBindingToken,
  type ProviderCredentialState,
} from '../../shared/schemas/credentials';
import type { Settings } from '../../shared/schemas/settings';
import { parseAwsCredentials } from './aws-sigv4';
import { ProviderError } from './errors';
import type { ProviderConfigService } from './provider-config-service';
import type {
  ProviderCredentialReconciliation,
  ProviderCredentialService,
} from './provider-credentials';
import type { ProviderService } from './provider-service';

type Configs = Pick<
  ProviderConfigService,
  'get' | 'save' | 'credentialEpoch' | 'advanceCredentialEpoch'
>;
type Credentials = Pick<
  ProviderCredentialService,
  'set' | 'status' | 'purgeProvider' | 'retainOnly' | 'unconfiguredStatus' | 'activateEpoch'
> &
  Partial<Pick<ProviderCredentialService, 'reconcileAll'>>;
type Providers = Pick<ProviderService, 'credentialBinding' | 'credentialPolicy'>;

interface BindingTokenState {
  readonly identity: string;
  readonly token: ProviderCredentialBindingToken;
}

export class ProviderMutationService {
  readonly #configs: Configs;
  readonly #credentials: Credentials;
  readonly #providers: Providers;
  readonly #bindingTokens = new Map<RunnableProviderId, BindingTokenState>();
  readonly #pendingReconciliation = new Set<RunnableProviderId>();
  #tail: Promise<void> = Promise.resolve();
  #accepting = true;

  constructor(configs: Configs, credentials: Credentials, providers: Providers) {
    this.#configs = configs;
    this.#credentials = credentials;
    this.#providers = providers;
  }

  saveConfig(configInput: RunnableProviderConfig): Promise<Settings> {
    const config = RunnableProviderConfigSchema.parse(configInput);
    return this.#run(() => this.#commitConfig(config));
  }

  saveConfigWithCredentialState(configInput: RunnableProviderConfig): Promise<{
    readonly settings: Settings;
    readonly credentialState: ProviderCredentialState;
  }> {
    const config = RunnableProviderConfigSchema.parse(configInput);
    return this.#run(async () => {
      const settings = await this.#commitConfig(config);
      // Mint and return the binding token in the same serialized mutation lease as the settings
      // commit. A second renderer cannot commit destination B between config A and this token.
      const state = this.#currentBindingState(config.providerId);
      const status =
        this.#providers.credentialPolicy(config.providerId) === 'none'
          ? this.#credentials.unconfiguredStatus(config.providerId)
          : this.#credentials.status(config.providerId, state.binding, state.epoch);
      return {
        settings,
        credentialState: ProviderCredentialStateSchema.parse({
          ...status,
          bindingToken: state.token,
        }),
      };
    });
  }

  setSecret(
    providerIdInput: RunnableProviderId,
    expectedBindingTokenInput: ProviderCredentialBindingToken,
    secret: string,
  ): Promise<ProviderCredentialState> {
    const providerId = RunnableProviderIdSchema.parse(providerIdInput);
    const expectedBindingToken =
      ProviderCredentialBindingTokenSchema.parse(expectedBindingTokenInput);
    return this.#run(async () => {
      const state = this.#currentBindingState(providerId);
      assertExpectedBinding(state.token, expectedBindingToken);
      if (this.#providers.credentialPolicy(providerId) === 'none') {
        throw new ProviderError('INVALID_CONFIG');
      }
      await this.#reconcileProvider(providerId, state.config, state.epoch);
      const canonicalSecret =
        providerId === 'bedrock' ? serializeAwsCredentials(parseAwsCredentials(secret)) : secret;
      const nextEpoch = incrementEpoch(state.epoch);
      await this.#configs.advanceCredentialEpoch(providerId, nextEpoch);
      this.#credentials.activateEpoch(providerId, nextEpoch);
      this.#bindingTokens.delete(providerId);
      const status = await this.#credentials.set(
        providerId,
        state.binding,
        canonicalSecret,
        nextEpoch,
      );
      return ProviderCredentialStateSchema.parse({
        ...status,
        bindingToken: this.#currentBindingState(providerId).token,
      });
    });
  }

  secretStatus(providerIdInput: RunnableProviderId): Promise<ProviderCredentialState> {
    const providerId = RunnableProviderIdSchema.parse(providerIdInput);
    return this.#run(async () => {
      const state = this.#currentBindingState(providerId);
      if (this.#providers.credentialPolicy(providerId) === 'none') {
        await this.#credentials.purgeProvider(providerId);
        return ProviderCredentialStateSchema.parse({
          ...this.#credentials.unconfiguredStatus(providerId),
          bindingToken: state.token,
        });
      }
      await this.#reconcileProvider(providerId, state.config, state.epoch);
      const status = this.#credentials.status(providerId, state.binding, state.epoch);
      return ProviderCredentialStateSchema.parse({ ...status, bindingToken: state.token });
    });
  }

  deleteSecret(
    providerIdInput: RunnableProviderId,
    expectedBindingTokenInput: ProviderCredentialBindingToken,
  ): Promise<ProviderCredentialState> {
    const providerId = RunnableProviderIdSchema.parse(providerIdInput);
    const expectedBindingToken =
      ProviderCredentialBindingTokenSchema.parse(expectedBindingTokenInput);
    return this.#run(async () => {
      const state = this.#currentBindingState(providerId);
      assertExpectedBinding(state.token, expectedBindingToken);
      const nextEpoch = incrementEpoch(state.epoch);
      await this.#configs.advanceCredentialEpoch(providerId, nextEpoch);
      this.#credentials.activateEpoch(providerId, nextEpoch);
      this.#bindingTokens.delete(providerId);
      await this.#credentials.purgeProvider(providerId);
      return ProviderCredentialStateSchema.parse({
        ...this.#credentials.unconfiguredStatus(providerId),
        bindingToken: this.#currentBindingState(providerId).token,
      });
    });
  }

  reconcileAll(): Promise<void> {
    return this.#run(async () => {
      if (this.#credentials.reconcileAll === undefined) {
        for (const providerId of RUNNABLE_PROVIDER_IDS) {
          try {
            const state = this.#currentBindingState(providerId);
            await this.#reconcileProvider(providerId, state.config, state.epoch);
          } catch {
            // Incomplete provider drafts have no usable credential binding yet. Cleanup is retried
            // after the next valid save/status operation for that provider.
          }
        }
        return;
      }

      const reconciliation: ProviderCredentialReconciliation[] = [];
      for (const providerId of RUNNABLE_PROVIDER_IDS) {
        try {
          const state = this.#currentBindingState(providerId);
          reconciliation.push({
            providerId,
            endpointBinding:
              this.#providers.credentialPolicy(providerId) === 'none' ? null : state.binding,
            credentialEpoch: state.epoch,
          });
        } catch {
          // Keep credentials for incomplete drafts physically intact but unreachable. A later
          // valid save/status operation will reconcile that provider.
        }
      }
      try {
        await this.#credentials.reconcileAll(reconciliation);
        for (const { providerId } of reconciliation) {
          this.#pendingReconciliation.delete(providerId);
        }
      } catch {
        // Epoch activation above is authoritative. One best-effort bulk vault rewrite replaces
        // provider-by-provider startup rewrites without making obsolete credentials reachable.
        for (const { providerId } of reconciliation) {
          this.#pendingReconciliation.add(providerId);
        }
      }
    });
  }

  stopAccepting(): void {
    this.#accepting = false;
  }

  async drain(): Promise<void> {
    await this.#tail;
  }

  async #commitConfig(config: RunnableProviderConfig): Promise<Settings> {
    let previousBinding: string | null = null;
    try {
      previousBinding = this.#providers.credentialBinding(
        RunnableProviderConfigSchema.parse(this.#configs.get(config.providerId)),
      );
    } catch {
      // A provider without a complete saved draft has no prior destination.
    }
    const nextBinding = this.#providers.credentialBinding(config);
    const previousEpoch = this.#configs.credentialEpoch(config.providerId);
    const nextEpoch =
      previousBinding !== null && previousBinding !== nextBinding
        ? incrementEpoch(previousEpoch)
        : previousEpoch;

    // Config and epoch commit atomically in settings. A failed settings write leaves the
    // previous epoch and credential active. A successful destination change makes every
    // older vault record unreachable before best-effort physical cleanup begins.
    const saved = await this.#configs.save(config, nextEpoch);
    this.#credentials.activateEpoch(config.providerId, nextEpoch);
    if (nextEpoch !== previousEpoch) this.#bindingTokens.delete(config.providerId);
    await this.#reconcileProvider(config.providerId, config, nextEpoch);
    return saved;
  }

  #currentBindingState(providerId: RunnableProviderId) {
    const config = RunnableProviderConfigSchema.parse(this.#configs.get(providerId));
    const binding = this.#providers.credentialBinding(config);
    const epoch = this.#configs.credentialEpoch(providerId);
    this.#credentials.activateEpoch(providerId, epoch);
    const identity = `${String(epoch)}\0${binding}`;
    let tokenState = this.#bindingTokens.get(providerId);
    if (tokenState?.identity !== identity) {
      tokenState = {
        identity,
        token: ProviderCredentialBindingTokenSchema.parse(randomUUID()),
      };
      this.#bindingTokens.set(providerId, tokenState);
    }
    return { config, binding, epoch, token: tokenState.token } as const;
  }

  async #reconcileProvider(
    providerId: RunnableProviderId,
    config: RunnableProviderConfig,
    epoch: number,
  ): Promise<void> {
    try {
      if (this.#providers.credentialPolicy(providerId) === 'none') {
        await this.#credentials.purgeProvider(providerId);
      } else {
        await this.#credentials.retainOnly(
          providerId,
          this.#providers.credentialBinding(config),
          epoch,
        );
      }
      this.#pendingReconciliation.delete(providerId);
    } catch {
      // Epoch isolation is authoritative; physical deletion can be retried without ever making
      // an obsolete record active again, including after restart.
      this.#pendingReconciliation.add(providerId);
    }
  }

  #run<Result>(operation: () => Promise<Result> | Result): Promise<Result> {
    if (!this.#accepting) return Promise.reject(new ProviderError('UNAVAILABLE'));
    const result = this.#tail.then(operation);
    this.#tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function assertExpectedBinding(
  current: ProviderCredentialBindingToken,
  expected: ProviderCredentialBindingToken,
): void {
  if (current !== expected) throw new ProviderError('STALE_CONFIG');
}

function incrementEpoch(epoch: number): number {
  if (!Number.isSafeInteger(epoch) || epoch < 0 || epoch === Number.MAX_SAFE_INTEGER) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return epoch + 1;
}
