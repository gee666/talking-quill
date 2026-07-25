import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import { ProviderCredentialService } from '../../app/src/main/providers/provider-credentials';
import { ProviderMutationService } from '../../app/src/main/providers/provider-mutation-service';
import { ProviderOperationCoordinator } from '../../app/src/main/providers/provider-operation-coordinator';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';
import type {
  JsonTransport,
  JsonTransportRequest,
  JsonTransportResponse,
} from '../../app/src/main/providers/json-transport';
import {
  CredentialVault,
  type SafeStorageAdapter,
} from '../../app/src/main/persistence/credential-vault';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

class FakeSafeStorage implements SafeStorageAdapter {
  isEncryptionAvailable() {
    return true;
  }
  encryptString(plainText: string) {
    return Buffer.from(`cipher:${Buffer.from(plainText).toString('base64')}`);
  }
  decryptString(encrypted: Buffer) {
    return Buffer.from(encrypted.toString().slice('cipher:'.length), 'base64').toString();
  }
}

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

async function persistence() {
  const directory = await createTestDirectory('provider-application');
  directories.push(directory);
  const settings = new SettingsStore(join(directory, 'settings.json'));
  const vault = new CredentialVault(join(directory, 'credentials.enc'), new FakeSafeStorage());
  await Promise.all([settings.initialize(), vault.initialize()]);
  return { directory, settings, vault };
}

describe('provider application integration', () => {
  it('derives vault IDs in main and returns status without IDs or values', async () => {
    const { directory, vault } = await persistence();
    const credentials = new ProviderCredentialService(vault);
    const secret = 'provider-canary-value';

    const binding = 'https://api.openai.com/v1/';
    const status = await credentials.set('openai', binding, secret);

    expect(status).toMatchObject({ providerId: 'openai', configured: true });
    expect(typeof status.updatedAt).toBe('number');
    expect(JSON.stringify(status)).not.toContain(secret);
    expect(JSON.stringify(status)).not.toContain('provider.openai.credential');
    const file = await readFile(join(directory, 'credentials.enc'), 'utf8');
    expect(file).toContain('p.openai.');
    expect(file).not.toContain(secret);
    expect(credentials.getCredential('openai', binding)).toBe(secret);
    expect(credentials.getCredential('openai', 'https://other.example/v1/')).toBeNull();
    await credentials.set('openai', 'https://other.example/v1/', 'replacement');
    expect(credentials.getCredential('openai', binding)).toBeNull();
    expect(credentials.getCredential('openai', 'https://other.example/v1/')).toBe('replacement');
    await credentials.deleteBinding('openai', binding);
    expect(credentials.getCredential('openai', 'https://other.example/v1/')).toBe('replacement');
    expect(await credentials.delete('openai', 'https://other.example/v1/')).toMatchObject({
      providerId: 'openai',
      configured: false,
    });
  });

  it('isolates credential epochs and removes obsolete records during reconciliation', async () => {
    const { vault } = await persistence();
    const credentials = new ProviderCredentialService(vault);
    const binding = 'https://api.openai.com/v1/';

    await credentials.set('openai', binding, 'epoch-zero-secret', 0);
    credentials.activateEpoch('openai', 1);
    expect(credentials.getCredential('openai', binding)).toBeNull();
    expect(credentials.status('openai', binding, 1).configured).toBe(false);

    await credentials.set('openai', binding, 'epoch-one-secret', 1);
    expect(credentials.getCredential('openai', binding)).toBe('epoch-one-secret');
    await credentials.retainOnly('openai', binding, 1);
    credentials.activateEpoch('openai', 0);
    expect(credentials.getCredential('openai', binding)).toBeNull();
    credentials.activateEpoch('openai', 1);
    expect(credentials.getCredential('openai', binding)).toBe('epoch-one-secret');
  });

  it('encrypts structured Bedrock credentials and binds them to the signing region', async () => {
    const { directory, vault } = await persistence();
    const credentials = new ProviderCredentialService(vault);
    const serialized = serializeAwsCredentials({
      accessKeyId: 'AKIDVAULTEXAMPLE12',
      secretAccessKey: 'vault-secret-access-key-example',
      sessionToken: 'vault-session-token-example',
    });

    await credentials.set('bedrock', 'aws-sigv4:us-west-2', serialized);

    expect(credentials.getCredential('bedrock', 'aws-sigv4:us-west-2')).toBe(serialized);
    expect(credentials.getCredential('bedrock', 'aws-sigv4:eu-west-1')).toBeNull();
    const file = await readFile(join(directory, 'credentials.enc'), 'utf8');
    expect(file).not.toContain('AKIDVAULTEXAMPLE12');
    expect(file).not.toContain('vault-secret-access-key-example');
    expect(file).not.toContain('vault-session-token-example');
  });

  it('never sends an endpoint-A credential after configuration moves to endpoint B', async () => {
    const { vault } = await persistence();
    const credentials = new ProviderCredentialService(vault);
    const requests: JsonTransportRequest[] = [];
    const transport: JsonTransport = {
      classify: () => Promise.resolve('local'),
      request: (request) => {
        requests.push(request);
        return Promise.resolve({
          status: 200,
          destination: 'local',
          body: { data: [{ id: 'model' }] },
        });
      },
    };
    const service = new ProviderService(new ProviderRegistry({ transport }), credentials);
    const endpointA = {
      providerId: 'generic-openai' as const,
      baseUrl: 'http://127.0.0.1:4321/v1',
    };
    await credentials.set(
      'generic-openai',
      service.credentialBinding(endpointA),
      'endpoint-a-secret',
    );
    await service.listModels(endpointA, new AbortController().signal);
    expect(requests.at(-1)?.headers?.authorization).toBe('Bearer endpoint-a-secret');

    const endpointB = { ...endpointA, baseUrl: 'http://127.0.0.1:9876/v1' };
    await service.listModels(endpointB, new AbortController().signal);
    expect(requests.at(-1)?.headers?.authorization).toBeUndefined();
    expect(requests.at(-1)?.credentialed).toBe(false);
  });

  it('saves selected non-secret configuration through the strict settings service', async () => {
    const { settings } = await persistence();
    const configs = new ProviderConfigService(settings);

    const saved = await configs.save({
      providerId: 'generic-openai',
      baseUrl: 'http://127.0.0.1:4321/v1',
      modelId: 'manual-model',
      contextWindow: 8_192,
    });

    expect(saved.smartProcessing.selectedProviderId).toBe('generic-openai');
    expect(configs.get('generic-openai').contextWindow).toBe(8_192);

    const replaced = await configs.save({
      providerId: 'generic-openai',
      baseUrl: 'http://127.0.0.1:4321/v1',
      modelId: 'manual-model',
    });

    expect(configs.get('generic-openai')).toEqual({
      providerId: 'generic-openai',
      baseUrl: 'http://127.0.0.1:4321/v1',
      modelId: 'manual-model',
    });
    expect(JSON.stringify(replaced)).not.toMatch(/secret|apiKey/i);
    await expect(
      configs.save({ providerId: 'anthropic', modelId: 'claude-sonnet-4-6' }),
    ).resolves.toMatchObject({ smartProcessing: { selectedProviderId: 'anthropic' } });
    await expect(configs.save({ providerId: 'bedrock' } as never)).rejects.toThrow();
  });

  it('commits an epoch before best-effort cleanup and reconciles on the next status read', async () => {
    const { settings, vault } = await persistence();
    const configs = new ProviderConfigService(settings);
    const credentials = new ProviderCredentialService(vault);
    const providers = new ProviderService(new ProviderRegistry(), credentials);
    const endpointA = {
      providerId: 'generic-openai' as const,
      baseUrl: 'http://127.0.0.1:4321/v1',
    };
    const endpointB = { ...endpointA, baseUrl: 'http://127.0.0.1:9876/v1' };
    await configs.save(endpointA);
    await credentials.set(
      'generic-openai',
      providers.credentialBinding(endpointA),
      'endpoint-a-secret',
    );
    const retainOnly = vi
      .spyOn(credentials, 'retainOnly')
      .mockRejectedValueOnce(new Error('vault cleanup failed'));
    const mutations = new ProviderMutationService(configs, credentials, providers);
    const changed: SettingsStore[] = [];
    settings.subscribe(() => changed.push(settings));

    await expect(mutations.saveConfig(endpointB)).resolves.toBeDefined();

    expect(changed).toHaveLength(1);
    expect(configs.get('generic-openai')).toEqual(endpointB);
    expect(configs.credentialEpoch('generic-openai')).toBe(1);
    expect(
      credentials.getCredential('generic-openai', providers.credentialBinding(endpointA)),
    ).toBeNull();
    const status = await mutations.secretStatus('generic-openai');
    expect(status.configured).toBe(false);
    expect(typeof status.bindingToken).toBe('string');
    expect(retainOnly).toHaveBeenCalledTimes(2);
  });

  it('filters every Bedrock credential component without matching unsafe partial strings', async () => {
    const aws = {
      accessKeyId: 'AKIDECHOFILTER1234',
      secretAccessKey: 'echo-filter-secret-access-key',
      sessionToken: 'echo-filter-session-token',
    };
    const credential = serializeAwsCredentials(aws);
    let completion = 'safe completion';
    const transport: JsonTransport = {
      classify: () => Promise.resolve('cloud'),
      request: (request): Promise<JsonTransportResponse> => {
        const url = String(request.url);
        if (url.includes('foundation-models')) {
          return Promise.resolve({
            status: 200,
            destination: 'cloud',
            body: {
              modelSummaries: [
                {
                  modelId: 'safe-AKIDECH',
                  modelName: 'safe partial identifier',
                  inputModalities: ['TEXT'],
                  outputModalities: ['TEXT'],
                  inferenceTypesSupported: ['ON_DEMAND'],
                },
                ...Object.values(aws).map((value) => ({
                  modelId: value,
                  modelName: `echo-${value}`,
                  inputModalities: ['TEXT'],
                  outputModalities: ['TEXT'],
                  inferenceTypesSupported: ['ON_DEMAND'],
                })),
              ],
            },
          });
        }
        if (url.includes('inference-profiles')) {
          return Promise.resolve({
            status: 200,
            destination: 'cloud',
            body: { inferenceProfileSummaries: [] },
          });
        }
        return Promise.resolve({
          status: 200,
          destination: 'cloud',
          body: {
            stopReason: 'end_turn',
            output: { message: { role: 'assistant', content: [{ text: completion }] } },
          },
        });
      },
    };
    const service = new ProviderService(new ProviderRegistry({ transport }), {
      getCredential: () => credential,
    });
    const config = {
      providerId: 'bedrock' as const,
      region: 'us-west-2',
      modelId: 'safe-AKIDECH',
    };

    await expect(service.listModels(config, new AbortController().signal)).resolves.toEqual([
      {
        id: 'safe-AKIDECH',
        name: 'safe partial identifier',
        contextWindow: null,
        vision: 'unsupported',
      },
    ]);
    for (const sensitive of Object.values(aws)) {
      completion = `unsafe ${sensitive}`;
      await expect(
        service.cleanTranscript(config, { input: 'raw' }, new AbortController().signal),
      ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
    }
  });

  it('redacts hostile model and completion echoes before service results leave main', async () => {
    const secret = 'hostile-echo-canary';
    const transport: JsonTransport = {
      classify: () => Promise.resolve('local'),
      request: (request: JsonTransportRequest): Promise<JsonTransportResponse> => {
        const authorization = request.headers?.authorization;
        expect(authorization).toBe(`Bearer ${secret}`);
        if (request.method === 'GET') {
          return Promise.resolve({
            status: 200,
            destination: 'local',
            body: {
              data: [
                { id: 'gpt-4.1-nano', name: 'gpt-4.1-nano' },
                { id: secret, name: `model-${secret}` },
              ],
            },
          });
        }
        return Promise.resolve({
          status: 200,
          destination: 'local',
          body: { choices: [{ message: { content: `cleaned ${secret}` } }] },
        });
      },
    };
    const service = new ProviderService(new ProviderRegistry({ transport }), {
      getCredential: () => secret,
    });
    const config = {
      providerId: 'generic-openai' as const,
      baseUrl: 'http://127.0.0.1:4321/v1',
      modelId: 'manual-model',
    };

    const models = await service.listModels(config, new AbortController().signal);
    const completion = service.cleanTranscript(
      config,
      { input: 'fixture transcript' },
      new AbortController().signal,
    );

    expect(JSON.stringify(models)).not.toContain(secret);
    expect(models).toEqual([
      {
        id: 'gpt-4.1-nano',
        name: 'gpt-4.1-nano',
        contextWindow: 4_096,
        vision: 'unknown',
      },
    ]);
    await expect(completion).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });

  it('owns cancellable operations per renderer and aborts them on destruction/dispose', async () => {
    const coordinator = new ProviderOperationCoordinator();
    const destroyedListeners = new Set<() => void>();
    const owner = {
      webContentsId: 7,
      onDestroyed: (listener: () => void) => {
        destroyedListeners.add(listener);
        return () => destroyedListeners.delete(listener);
      },
    };
    const abortSeen = vi.fn();
    const pending = coordinator.run(
      owner,
      'operation-1',
      (signal) =>
        new Promise<never>((_resolve, reject) => {
          signal.addEventListener(
            'abort',
            () => {
              abortSeen();
              reject(new Error('aborted'));
            },
            { once: true },
          );
        }),
    );

    expect(coordinator.cancel(8, 'operation-1')).toBe(false);
    expect(coordinator.cancel(7, 'operation-1')).toBe(true);
    await expect(pending).rejects.toThrow('aborted');
    expect(abortSeen).toHaveBeenCalledOnce();

    const destroyedPending = coordinator.run(
      owner,
      'operation-2',
      (signal) =>
        new Promise<never>((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(new Error('renderer gone')), {
            once: true,
          });
        }),
    );
    for (const listener of [...destroyedListeners]) listener();
    await expect(destroyedPending).rejects.toThrow('renderer gone');

    vi.useFakeTimers();
    const hardTimeout = expect(
      coordinator.run(owner, 'operation-hard-timeout', () => new Promise<never>(() => undefined)),
    ).rejects.toMatchObject({ publicError: { code: 'TIMEOUT' } });
    await vi.advanceTimersByTimeAsync(120_000);
    await hardTimeout;
    vi.useRealTimers();

    coordinator.dispose();
    await expect(
      coordinator.run(owner, 'operation-3', () => Promise.resolve('never')),
    ).rejects.toThrow('shutting down');
  });
});
