import { createHash } from 'node:crypto';
import {
  CredentialSecretSchema,
  ProviderCredentialStatusSchema,
  type ProviderCredentialStatus,
} from '../../shared/schemas/credentials';
import {
  RunnableProviderIdSchema,
  type ProviderId,
  type RunnableProviderId,
} from '../../shared/schemas/providers';
import type { CredentialVault } from '../persistence/credential-vault';
import type { CredentialResolver } from './contracts';

const MAX_BINDING_LENGTH = 4_096;

export interface ProviderCredentialReconciliation {
  readonly providerId: RunnableProviderId;
  readonly endpointBinding: string | null;
  readonly credentialEpoch: number;
}

export class ProviderCredentialService implements CredentialResolver {
  readonly #vault: CredentialVault;
  readonly #activeEpochs = new Map<RunnableProviderId, number>();

  constructor(vault: CredentialVault) {
    this.#vault = vault;
  }

  getCredential(providerId: ProviderId, endpointBinding: string): string | null {
    const id = RunnableProviderIdSchema.parse(providerId);
    return this.#vault.getForMain(
      vaultId(id, parseBinding(endpointBinding), this.#activeEpochs.get(id) ?? 0),
    );
  }

  activateEpoch(providerId: RunnableProviderId, credentialEpoch: number): void {
    const id = RunnableProviderIdSchema.parse(providerId);
    this.#activeEpochs.set(id, zCredentialEpoch(credentialEpoch));
  }

  async set(
    providerId: RunnableProviderId,
    endpointBinding: string,
    secret: string,
    credentialEpoch = 0,
  ): Promise<ProviderCredentialStatus> {
    const id = RunnableProviderIdSchema.parse(providerId);
    const binding = parseBinding(endpointBinding);
    const parsedSecret = CredentialSecretSchema.parse(secret);
    const status = await this.#vault.replaceByPrefixes(
      [vaultPrefix(id), legacyVaultId(id)],
      vaultId(id, binding, credentialEpoch),
      parsedSecret,
    );
    return ProviderCredentialStatusSchema.parse({
      providerId: id,
      configured: status.configured,
      updatedAt: status.updatedAt,
    });
  }

  status(
    providerId: RunnableProviderId,
    endpointBinding: string,
    credentialEpoch = 0,
  ): ProviderCredentialStatus {
    const id = RunnableProviderIdSchema.parse(providerId);
    const status = this.#vault.status(vaultId(id, parseBinding(endpointBinding), credentialEpoch));
    return ProviderCredentialStatusSchema.parse({
      providerId: id,
      configured: status.configured,
      updatedAt: status.updatedAt,
    });
  }

  async delete(
    providerId: RunnableProviderId,
    endpointBinding: string,
  ): Promise<ProviderCredentialStatus> {
    return this.deleteBinding(providerId, endpointBinding);
  }

  async deleteBinding(
    providerId: RunnableProviderId,
    endpointBinding: string,
  ): Promise<ProviderCredentialStatus> {
    const id = RunnableProviderIdSchema.parse(providerId);
    const binding = parseBinding(endpointBinding);
    const epoch = this.#activeEpochs.get(id) ?? 0;
    await this.#vault.delete(vaultId(id, binding, epoch));
    return this.status(id, binding, epoch);
  }

  async retainOnly(
    providerId: RunnableProviderId,
    endpointBinding: string,
    credentialEpoch: number,
  ): Promise<void> {
    const id = RunnableProviderIdSchema.parse(providerId);
    const binding = parseBinding(endpointBinding);
    await Promise.all([
      this.#vault.deleteByPrefixExcept(vaultPrefix(id), vaultId(id, binding, credentialEpoch)),
      this.#vault.delete(legacyVaultId(id)),
    ]);
  }

  async purgeProvider(providerId: RunnableProviderId): Promise<void> {
    const id = RunnableProviderIdSchema.parse(providerId);
    await this.#purgeProvider(id);
  }

  async reconcileAll(entries: readonly ProviderCredentialReconciliation[]): Promise<void> {
    const prefixes: string[] = [];
    const exactIds: string[] = [];
    const retainedIds: string[] = [];
    const seen = new Set<RunnableProviderId>();
    for (const entry of entries) {
      const providerId = RunnableProviderIdSchema.parse(entry.providerId);
      if (seen.has(providerId)) throw new Error('Duplicate provider credential reconciliation');
      seen.add(providerId);
      const epoch = zCredentialEpoch(entry.credentialEpoch);
      prefixes.push(vaultPrefix(providerId));
      exactIds.push(legacyVaultId(providerId));
      if (entry.endpointBinding !== null) {
        retainedIds.push(vaultId(providerId, parseBinding(entry.endpointBinding), epoch));
      }
    }
    await this.#vault.reconcileRecords({ prefixes, exactIds, retainedIds });
  }

  unconfiguredStatus(providerId: RunnableProviderId): ProviderCredentialStatus {
    const id = RunnableProviderIdSchema.parse(providerId);
    return ProviderCredentialStatusSchema.parse({
      providerId: id,
      configured: false,
      updatedAt: null,
    });
  }

  async purgeImpossible(providerId: RunnableProviderId, endpointBinding: string): Promise<void> {
    const id = RunnableProviderIdSchema.parse(providerId);
    parseBinding(endpointBinding);
    await this.#purgeProvider(id);
  }

  async #purgeProvider(providerId: RunnableProviderId): Promise<void> {
    await Promise.all([
      this.#vault.deleteByPrefix(vaultPrefix(providerId)),
      this.#vault.delete(legacyVaultId(providerId)),
    ]);
  }
}

function parseBinding(binding: string): string {
  if (
    binding.length === 0 ||
    binding.length > MAX_BINDING_LENGTH ||
    binding.trim() !== binding ||
    hasControlCharacters(binding)
  ) {
    throw new Error('Invalid provider credential binding');
  }
  return binding;
}

function hasControlCharacters(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

function vaultPrefix(providerId: RunnableProviderId): string {
  return `p.${providerId}.`;
}

function vaultId(
  providerId: RunnableProviderId,
  endpointBinding: string,
  credentialEpoch: number,
): string {
  const epoch = zCredentialEpoch(credentialEpoch);
  const digestInput = epoch === 0 ? endpointBinding : `${String(epoch)}\0${endpointBinding}`;
  const digest = createHash('sha256').update(digestInput, 'utf8').digest('hex').slice(0, 40);
  return `p.${providerId}.${digest}`;
}

function zCredentialEpoch(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error('Invalid credential epoch');
  return value;
}

function legacyVaultId(providerId: RunnableProviderId): string {
  return `provider.${providerId}.credential`;
}
