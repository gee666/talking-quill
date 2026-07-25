import { chmod, open } from 'node:fs/promises';
import { z } from 'zod';
import {
  CredentialIdSchema,
  CredentialSecretSchema,
  CredentialStatusSchema,
  type CredentialStatus,
} from '../../shared/schemas/credentials';
import { preserveInvalidFile, writeJsonAtomic } from './atomic-json';

const VAULT_SCHEMA_VERSION = 1 as const;
const MAX_RECORDS = 64;
export const MAX_ENCRYPTED_CREDENTIAL_BYTES = 128 * 1024;
const MAX_ENCRYPTED_BASE64_CHARACTERS = Math.ceil(MAX_ENCRYPTED_CREDENTIAL_BYTES / 3) * 4;
export const MAX_VAULT_FILE_BYTES = MAX_RECORDS * (MAX_ENCRYPTED_BASE64_CHARACTERS + 512) + 1024;

const VaultRecordSchema = z
  .object({
    encrypted: z.string().min(4).max(MAX_ENCRYPTED_BASE64_CHARACTERS).pipe(z.base64()),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
const VaultSchema = z
  .object({
    schemaVersion: z.literal(VAULT_SCHEMA_VERSION),
    records: z.record(CredentialIdSchema, VaultRecordSchema),
  })
  .strict();
type VaultData = z.infer<typeof VaultSchema>;

export interface SafeStorageAdapter {
  isEncryptionAvailable(): boolean;
  encryptString(plainText: string): Buffer;
  decryptString(encrypted: Buffer): string;
}

export class VaultUnavailableError extends Error {
  constructor() {
    super('Secure credential storage is unavailable');
    this.name = 'VaultUnavailableError';
  }
}

export class CredentialVault {
  readonly #path: string;
  readonly #safeStorage: SafeStorageAdapter;
  #data: VaultData = { schemaVersion: VAULT_SCHEMA_VERSION, records: {} };
  #faulted = false;
  #writeQueue: Promise<void> = Promise.resolve();

  constructor(path: string, safeStorage: SafeStorageAdapter) {
    this.#path = path;
    this.#safeStorage = safeStorage;
  }

  async initialize(): Promise<void> {
    if (!this.#safeStorage.isEncryptionAvailable()) {
      this.#faulted = true;
      return;
    }
    let source: string | null;
    try {
      source = await readBoundedVault(this.#path);
    } catch (error) {
      if (!(error instanceof VaultPayloadTooLargeError)) throw error;
      this.#faulted = true;
      await preserveInvalidFile(this.#path);
      return;
    }
    if (source === null) return;
    try {
      const input = JSON.parse(source) as unknown;
      assertVaultRecordCapacity(input);
      const parsed = VaultSchema.parse(input);
      const records: VaultData['records'] = {};
      for (const [id, record] of Object.entries(parsed.records)) {
        const encrypted = Buffer.from(record.encrypted, 'base64');
        assertEncryptedSize(encrypted);
        let plainText: string | null = this.#safeStorage.decryptString(encrypted);
        try {
          if (CredentialSecretSchema.safeParse(plainText).success) records[id] = record;
        } finally {
          plainText = null;
        }
      }
      this.#data = VaultSchema.parse({ schemaVersion: VAULT_SCHEMA_VERSION, records });
      if (Object.keys(records).length !== Object.keys(parsed.records).length) {
        await this.#write(this.#data);
      }
    } catch {
      this.#faulted = true;
      await preserveInvalidFile(this.#path);
    }
  }

  async set(id: string, secret: string): Promise<CredentialStatus> {
    const parsedId = CredentialIdSchema.parse(id);
    const parsedSecret = CredentialSecretSchema.parse(secret);
    return this.#enqueueMutation(async () => {
      this.#assertOperational();
      if (
        !(parsedId in this.#data.records) &&
        Object.keys(this.#data.records).length >= MAX_RECORDS
      ) {
        throw new VaultUnavailableError();
      }
      const updatedAt = Date.now();
      const encryptedBuffer = this.#safeStorage.encryptString(parsedSecret);
      assertEncryptedSize(encryptedBuffer);
      const encrypted = encryptedBuffer.toString('base64');
      const next = VaultSchema.parse({
        schemaVersion: VAULT_SCHEMA_VERSION,
        records: { ...this.#data.records, [parsedId]: { encrypted, updatedAt } },
      });
      await this.#write(next);
      this.#data = next;
      return CredentialStatusSchema.parse({ id: parsedId, configured: true, updatedAt });
    });
  }

  status(id: string): CredentialStatus {
    this.#assertOperational();
    const parsedId = CredentialIdSchema.parse(id);
    const record = this.#data.records[parsedId];
    return CredentialStatusSchema.parse({
      id: parsedId,
      configured: record !== undefined,
      updatedAt: record?.updatedAt ?? null,
    });
  }

  async delete(id: string): Promise<boolean> {
    const parsedId = CredentialIdSchema.parse(id);
    return this.#enqueueMutation(() => this.#deleteWhere((recordId) => recordId === parsedId));
  }

  async replaceByPrefixes(
    prefixes: readonly string[],
    id: string,
    secret: string,
  ): Promise<CredentialStatus> {
    const parsedPrefixes = prefixes.map((prefix) => CredentialIdSchema.parse(prefix));
    const parsedId = CredentialIdSchema.parse(id);
    const parsedSecret = CredentialSecretSchema.parse(secret);
    return this.#enqueueMutation(async () => {
      this.#assertOperational();
      const updatedAt = Date.now();
      const encryptedBuffer = this.#safeStorage.encryptString(parsedSecret);
      assertEncryptedSize(encryptedBuffer);
      const records = Object.fromEntries(
        Object.entries(this.#data.records).filter(
          ([recordId]) => !parsedPrefixes.some((prefix) => recordId.startsWith(prefix)),
        ),
      );
      records[parsedId] = {
        encrypted: encryptedBuffer.toString('base64'),
        updatedAt,
      };
      const next = VaultSchema.parse({ schemaVersion: VAULT_SCHEMA_VERSION, records });
      await this.#write(next);
      this.#data = next;
      return CredentialStatusSchema.parse({ id: parsedId, configured: true, updatedAt });
    });
  }

  async deleteByPrefix(prefix: string): Promise<number> {
    return this.deleteByPrefixExcept(prefix, null);
  }

  async deleteByPrefixExcept(prefix: string, retainedId: string | null): Promise<number> {
    const parsedPrefix = CredentialIdSchema.parse(prefix);
    const parsedRetainedId = retainedId === null ? null : CredentialIdSchema.parse(retainedId);
    return this.#enqueueMutation(async () => {
      let deleted = 0;
      await this.#deleteWhere((recordId) => {
        const matches = recordId.startsWith(parsedPrefix) && recordId !== parsedRetainedId;
        if (matches) deleted += 1;
        return matches;
      });
      return deleted;
    });
  }

  getForMain(id: string): string | null {
    this.#assertOperational();
    const parsedId = CredentialIdSchema.parse(id);
    const record = this.#data.records[parsedId];
    if (record === undefined) return null;
    return this.#safeStorage.decryptString(Buffer.from(record.encrypted, 'base64'));
  }

  async flush(): Promise<void> {
    await this.#writeQueue;
  }

  #enqueueMutation<Result>(operation: () => Promise<Result>): Promise<Result> {
    const mutation = this.#writeQueue.then(operation);
    this.#writeQueue = mutation.then(
      () => undefined,
      () => undefined,
    );
    return mutation;
  }

  async #deleteWhere(predicate: (recordId: string) => boolean): Promise<boolean> {
    this.#assertOperational();
    const records = Object.fromEntries(
      Object.entries(this.#data.records).filter(([recordId]) => !predicate(recordId)),
    );
    if (Object.keys(records).length === Object.keys(this.#data.records).length) return false;
    const next = VaultSchema.parse({ schemaVersion: VAULT_SCHEMA_VERSION, records });
    await this.#write(next);
    this.#data = next;
    return true;
  }

  async #write(data: VaultData): Promise<void> {
    await writeJsonAtomic(this.#path, data);
    await chmod(this.#path, 0o600).catch(() => undefined);
  }

  #assertOperational(): void {
    if (this.#faulted || !this.#safeStorage.isEncryptionAvailable()) {
      throw new VaultUnavailableError();
    }
  }
}

class VaultPayloadTooLargeError extends Error {}

async function readBoundedVault(path: string): Promise<string | null> {
  let handle;
  try {
    handle = await open(path, 'r');
  } catch (error) {
    if (isErrnoException(error) && error.code === 'ENOENT') return null;
    throw error;
  }

  try {
    const buffer = Buffer.allocUnsafe(MAX_VAULT_FILE_BYTES + 1);
    let bytesRead = 0;
    while (bytesRead < buffer.length) {
      const result = await handle.read(buffer, bytesRead, buffer.length - bytesRead, bytesRead);
      if (result.bytesRead === 0) break;
      bytesRead += result.bytesRead;
    }
    if (bytesRead > MAX_VAULT_FILE_BYTES) throw new VaultPayloadTooLargeError();
    return buffer.toString('utf8', 0, bytesRead);
  } finally {
    await handle.close();
  }
}

function isErrnoException(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}

function assertVaultRecordCapacity(input: unknown): void {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return;
  const records = (input as Readonly<Record<string, unknown>>).records;
  if (typeof records !== 'object' || records === null || Array.isArray(records)) return;
  if (Object.keys(records).length > MAX_RECORDS) throw new VaultUnavailableError();
}

function assertEncryptedSize(encrypted: Buffer): void {
  if (encrypted.length === 0 || encrypted.length > MAX_ENCRYPTED_CREDENTIAL_BYTES) {
    throw new VaultUnavailableError();
  }
}
