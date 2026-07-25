import { readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  CredentialVault,
  MAX_ENCRYPTED_CREDENTIAL_BYTES,
  MAX_VAULT_FILE_BYTES,
  VaultUnavailableError,
  type SafeStorageAdapter,
} from '../../app/src/main/persistence/credential-vault';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

class FakeSafeStorage implements SafeStorageAdapter {
  constructor(private readonly available = true) {}
  isEncryptionAvailable() {
    return this.available;
  }
  encryptString(plainText: string) {
    return Buffer.from(`encrypted:${Buffer.from(plainText).toString('base64')}`);
  }
  decryptString(encrypted: Buffer) {
    const value = encrypted.toString();
    if (!value.startsWith('encrypted:')) throw new Error('Ciphertext rejected');
    return Buffer.from(value.slice('encrypted:'.length), 'base64').toString();
  }
}

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

async function vaultPath() {
  const directory = await createTestDirectory('vault');
  directories.push(directory);
  return join(directory, 'credentials.enc');
}

describe('CredentialVault', () => {
  it('writes ciphertext, exposes status only, and decrypts only for main services', async () => {
    const path = await vaultPath();
    const vault = new CredentialVault(path, new FakeSafeStorage());
    await vault.initialize();
    const status = await vault.set('openai', 'top-secret-value');
    expect(status.configured).toBe(true);
    expect(await readFile(path, 'utf8')).not.toContain('top-secret-value');
    expect(vault.status('openai').configured).toBe(true);
    expect(vault.getForMain('openai')).toBe('top-secret-value');
    expect(await vault.delete('openai')).toBe(true);
    expect(vault.status('openai').configured).toBe(false);
  });

  it('serializes concurrent mutations without losing records', async () => {
    const path = await vaultPath();
    const vault = new CredentialVault(path, new FakeSafeStorage());
    await vault.initialize();
    await Promise.all([
      vault.set('first', 'secret-one'),
      vault.set('second', 'secret-two'),
      vault.set('third', 'secret-three'),
    ]);
    await vault.flush();

    const restarted = new CredentialVault(path, new FakeSafeStorage());
    await restarted.initialize();
    expect(restarted.getForMain('first')).toBe('secret-one');
    expect(restarted.getForMain('second')).toBe('secret-two');
    expect(restarted.getForMain('third')).toBe('secret-three');
    await Promise.all([restarted.delete('first'), restarted.set('fourth', 'secret-four')]);
    expect(restarted.status('first').configured).toBe(false);
    expect(restarted.getForMain('fourth')).toBe('secret-four');
  });

  it('fails closed when OS encryption is unavailable', async () => {
    const vault = new CredentialVault(await vaultPath(), new FakeSafeStorage(false));
    await vault.initialize();
    await expect(vault.set('openai', 'secret-value')).rejects.toBeInstanceOf(VaultUnavailableError);
    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
  });

  it('fails closed for valid Base64 that safeStorage rejects', async () => {
    const path = await vaultPath();
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        records: {
          openai: {
            encrypted: Buffer.from('not-safe-storage-ciphertext').toString('base64'),
            updatedAt: 1,
          },
        },
      }),
      'utf8',
    );
    const vault = new CredentialVault(path, new FakeSafeStorage());
    await vault.initialize();

    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
    expect((await readdir(dirname(path))).some((name) => name.endsWith('.invalid'))).toBe(true);
  });

  it('drops a legacy decrypted value that is not a valid credential secret', async () => {
    const path = await vaultPath();
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        records: {
          openai: {
            encrypted: Buffer.from('ciphertext').toString('base64'),
            updatedAt: 1,
          },
        },
      }),
      'utf8',
    );
    const vault = new CredentialVault(path, {
      isEncryptionAvailable: () => true,
      encryptString: (plainText) => Buffer.from(plainText),
      decryptString: () => '',
    });

    await vault.initialize();

    expect(vault.status('openai')).toMatchObject({ configured: false, updatedAt: null });
  });

  it('rejects oversized ciphertext before attempting decryption', async () => {
    const path = await vaultPath();
    const decryptString = vi.fn(() => 'secret');
    const adapter: SafeStorageAdapter = {
      isEncryptionAvailable: () => true,
      encryptString: (plainText) => Buffer.from(plainText),
      decryptString,
    };
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        records: {
          openai: {
            encrypted: Buffer.alloc(MAX_ENCRYPTED_CREDENTIAL_BYTES + 1, 1).toString('base64'),
            updatedAt: 1,
          },
        },
      }),
      'utf8',
    );
    const vault = new CredentialVault(path, adapter);
    await vault.initialize();

    expect(decryptString).not.toHaveBeenCalled();
    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
  });

  it('rejects oversized encryption output without changing existing records', async () => {
    const path = await vaultPath();
    const encryptString = vi.fn((plainText: string) => Buffer.from(`encrypted:${plainText}`));
    const adapter: SafeStorageAdapter = {
      isEncryptionAvailable: () => true,
      encryptString,
      decryptString: (encrypted) => encrypted.toString().slice('encrypted:'.length),
    };
    const vault = new CredentialVault(path, adapter);
    await vault.initialize();
    await vault.set('existing', 'preserved-secret');
    encryptString.mockReturnValueOnce(Buffer.alloc(MAX_ENCRYPTED_CREDENTIAL_BYTES + 1, 1));

    await expect(vault.set('openai', 'secret-value')).rejects.toBeInstanceOf(VaultUnavailableError);
    expect(vault.getForMain('existing')).toBe('preserved-secret');
    expect(vault.status('openai').configured).toBe(false);
  });

  it('rejects more than the maximum record count before decrypting records', async () => {
    const path = await vaultPath();
    const records = Object.fromEntries(
      Array.from({ length: 65 }, (_, index) => [
        `credential-${String(index).padStart(2, '0')}`,
        { encrypted: Buffer.from('ciphertext').toString('base64'), updatedAt: 1 },
      ]),
    );
    await writeFile(path, JSON.stringify({ schemaVersion: 1, records }), 'utf8');
    const decryptString = vi.fn(() => 'valid-secret');
    const vault = new CredentialVault(path, {
      isEncryptionAvailable: () => true,
      encryptString: (plainText) => Buffer.from(plainText),
      decryptString,
    });

    await vault.initialize();

    expect(decryptString).not.toHaveBeenCalled();
    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
  });

  it('accepts the maximum valid serialized vault within the file bound', async () => {
    const path = await vaultPath();
    const encrypted = Buffer.alloc(MAX_ENCRYPTED_CREDENTIAL_BYTES, 1).toString('base64');
    const records = Object.fromEntries(
      Array.from({ length: 64 }, (_, index) => [
        `credential-${String(index).padStart(2, '0')}${'x'.repeat(51)}`,
        { encrypted, updatedAt: Number.MAX_SAFE_INTEGER },
      ]),
    );
    const serialized = `${JSON.stringify({ schemaVersion: 1, records }, null, 2)}\n`;
    expect(Buffer.byteLength(serialized)).toBeLessThanOrEqual(MAX_VAULT_FILE_BYTES);
    await writeFile(path, serialized, 'utf8');
    const decryptString = vi.fn(() => 'valid-secret');
    const vault = new CredentialVault(path, {
      isEncryptionAvailable: () => true,
      encryptString: (plainText) => Buffer.from(plainText),
      decryptString,
    });

    await vault.initialize();

    expect(decryptString).toHaveBeenCalledTimes(64);
    expect(vault.status(`credential-00${'x'.repeat(51)}`).configured).toBe(true);
  });

  it('bounds the serialized vault before parsing or decrypting', async () => {
    const path = await vaultPath();
    const decryptString = vi.fn(() => 'secret');
    await writeFile(path, Buffer.alloc(MAX_VAULT_FILE_BYTES + 1, 0x20));
    const vault = new CredentialVault(path, {
      isEncryptionAvailable: () => true,
      encryptString: (plainText) => Buffer.from(plainText),
      decryptString,
    });

    await vault.initialize();

    expect(decryptString).not.toHaveBeenCalled();
    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
  });

  it('preserves malformed vault data and remains faulted', async () => {
    const path = await vaultPath();
    await writeFile(path, '{broken', 'utf8');
    const vault = new CredentialVault(path, new FakeSafeStorage());
    await vault.initialize();
    expect(() => vault.status('openai')).toThrow(VaultUnavailableError);
    expect((await readdir(dirname(path))).some((name) => name.endsWith('.invalid'))).toBe(true);
  });
});
