import { describe, expect, it, vi } from 'vitest';
import { ProviderMutationService } from '../../app/src/main/providers/provider-mutation-service';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';
import type {
  ProviderConfig,
  RunnableProviderConfig,
  RunnableProviderId,
} from '../../app/src/shared/schemas/providers';

const endpointA = {
  providerId: 'generic-openai' as const,
  baseUrl: 'http://127.0.0.1:4001/v1',
};
const endpointB = {
  providerId: 'generic-openai' as const,
  baseUrl: 'http://127.0.0.1:4002/v1',
};

function harness() {
  let current: ProviderConfig = endpointA;
  let epoch = 0;
  let activeEpoch = 0;
  const records = new Map<string, string>([['generic-openai:0:A', 'old-key']]);
  const calls: string[] = [];
  const saveGate = deferred();
  let gateSaves = true;
  const binding = (config: ProviderConfig): string =>
    config.baseUrl === endpointA.baseUrl ? 'A' : 'B';
  const configs = {
    get: () => current,
    credentialEpoch: () => epoch,
    advanceCredentialEpoch: (
      providerId: RunnableProviderId,
      nextEpoch: number,
    ): Promise<Settings> => {
      calls.push(`epoch:${String(nextEpoch)}`);
      epoch = nextEpoch;
      return Promise.resolve({
        ...structuredClone(DEFAULT_SETTINGS),
        smartProcessing: {
          ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
          selectedProviderId: providerId,
          credentialEpochs: { [providerId]: epoch },
        },
      });
    },
    save: async (config: RunnableProviderConfig, nextEpoch?: number): Promise<Settings> => {
      calls.push(`save:start:${String(nextEpoch)}`);
      if (gateSaves) await saveGate.promise;
      current = config;
      epoch = nextEpoch ?? epoch;
      calls.push(`save:commit:${String(epoch)}`);
      const { providerId, ...draft } = config;
      return {
        ...structuredClone(DEFAULT_SETTINGS),
        smartProcessing: {
          selectedProviderId: providerId,
          providers: { [providerId]: draft },
          credentialEpochs: { [providerId]: epoch },
          piInstallationPath: null,
          onScreenAwarenessEnabled: false,
          visionOverrides: [],
        },
      };
    },
  };
  const credentials = {
    activateEpoch: (_providerId: RunnableProviderId, nextEpoch: number) => {
      activeEpoch = nextEpoch;
      calls.push(`activate:${String(nextEpoch)}`);
    },
    set: (
      providerId: RunnableProviderId,
      endpointBinding: string,
      secret: string,
      credentialEpoch = 0,
    ) => {
      calls.push(`set:${String(credentialEpoch)}:${endpointBinding}`);
      for (const key of [...records.keys()]) {
        if (key.startsWith(`${providerId}:`)) records.delete(key);
      }
      records.set(`${providerId}:${String(credentialEpoch)}:${endpointBinding}`, secret);
      return Promise.resolve({ providerId, configured: true, updatedAt: 1 } as const);
    },
    status: (providerId: RunnableProviderId, endpointBinding: string, credentialEpoch = 0) => ({
      providerId,
      configured: records.has(`${providerId}:${String(credentialEpoch)}:${endpointBinding}`),
      updatedAt: records.has(`${providerId}:${String(credentialEpoch)}:${endpointBinding}`)
        ? 1
        : null,
    }),
    retainOnly: (
      providerId: RunnableProviderId,
      endpointBinding: string,
      credentialEpoch: number,
    ) => {
      calls.push(`retain:${String(credentialEpoch)}:${endpointBinding}`);
      const retained = `${providerId}:${String(credentialEpoch)}:${endpointBinding}`;
      for (const key of [...records.keys()]) {
        if (key.startsWith(`${providerId}:`) && key !== retained) records.delete(key);
      }
      return Promise.resolve();
    },
    purgeProvider: (providerId: RunnableProviderId) => {
      calls.push(`purge:${providerId}`);
      for (const key of [...records.keys()]) {
        if (key.startsWith(`${providerId}:`)) records.delete(key);
      }
      return Promise.resolve();
    },
    unconfiguredStatus: (providerId: RunnableProviderId) => ({
      providerId,
      configured: false,
      updatedAt: null,
    }),
  };
  const providers = {
    credentialBinding: binding,
    credentialPolicy: () => 'required' as const,
  };
  const mutations = new ProviderMutationService(configs, credentials, providers);
  return {
    mutations,
    calls,
    configs,
    credentials,
    records,
    saveGate,
    setGateSaves(value: boolean) {
      gateSaves = value;
    },
    activeEpoch: () => activeEpoch,
  };
}

describe('ProviderMutationService credential binding', () => {
  it('atomically advances the credential epoch when the destination changes', async () => {
    const { mutations, calls, records, saveGate, activeEpoch } = harness();

    const save = mutations.saveConfig(endpointB);
    await vi.waitFor(() => expect(calls).toContain('save:start:1'));
    expect(activeEpoch()).toBe(0);
    expect(records.has('generic-openai:0:A')).toBe(true);

    saveGate.resolve();
    const saved = await save;

    expect(saved.smartProcessing.credentialEpochs['generic-openai']).toBe(1);
    expect(activeEpoch()).toBe(1);
    expect(records.size).toBe(0);
    expect(calls).toContain('retain:1:B');
  });

  it('keeps the old epoch active when settings persistence fails', async () => {
    const { mutations, configs, credentials, records, activeEpoch } = harness();
    const failure = new Error('settings persistence failed');
    vi.spyOn(configs, 'save').mockRejectedValueOnce(failure);
    const cleanup = vi.spyOn(credentials, 'retainOnly');

    await expect(mutations.saveConfig(endpointB)).rejects.toBe(failure);

    expect(configs.get()).toEqual(endpointA);
    expect(activeEpoch()).toBe(0);
    expect(records.get('generic-openai:0:A')).toBe('old-key');
    expect(cleanup).not.toHaveBeenCalled();
  });

  it('makes stale credentials unreachable even when physical reconciliation fails', async () => {
    const { mutations, credentials, records, saveGate, activeEpoch } = harness();
    vi.spyOn(credentials, 'retainOnly').mockRejectedValueOnce(new Error('vault unavailable'));

    const save = mutations.saveConfig(endpointB);
    saveGate.resolve();
    await expect(save).resolves.toBeDefined();

    expect(activeEpoch()).toBe(1);
    expect(records.get('generic-openai:0:A')).toBe('old-key');
    const state = await mutations.secretStatus('generic-openai');
    expect(state.configured).toBe(false);
    expect(credentials.retainOnly).toHaveBeenCalledTimes(2);
    expect(records.size).toBe(0);
  });

  it('rejects stale renderer tokens after a binding change and accepts the refreshed token', async () => {
    const { mutations, saveGate, records } = harness();
    const before = await mutations.secretStatus('generic-openai');
    const save = mutations.saveConfig(endpointB);
    saveGate.resolve();
    await save;

    await expect(
      mutations.setSecret('generic-openai', before.bindingToken, 'must-not-write'),
    ).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    expect(records.size).toBe(0);

    const current = await mutations.secretStatus('generic-openai');
    expect(current.bindingToken).not.toBe(before.bindingToken);
    const replaced = await mutations.setSecret('generic-openai', current.bindingToken, 'new-key');
    expect(replaced).toMatchObject({ configured: true });
    expect(replaced.bindingToken).not.toBe(current.bindingToken);
    expect([...records.entries()]).toEqual([['generic-openai:2:B', 'new-key']]);
  });

  it('rejects config-A secret after config-B using the token returned atomically with save A', async () => {
    const test = harness();
    const saveA = test.mutations.saveConfigWithCredentialState(endpointA);
    test.saveGate.resolve();
    const committedA = await saveA;
    test.setGateSaves(false);

    await test.mutations.saveConfig(endpointB);
    await expect(
      test.mutations.setSecret(
        'generic-openai',
        committedA.credentialState.bindingToken,
        'stale-A-secret',
      ),
    ).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    expect([...test.records.values()]).not.toContain('stale-A-secret');
  });

  it('requires the current token for deletion', async () => {
    const { mutations, records } = harness();
    const state = await mutations.secretStatus('generic-openai');
    await expect(
      mutations.deleteSecret('generic-openai', '00000000-0000-0000-0000-000000000000'),
    ).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    expect(records.size).toBe(1);

    await expect(
      mutations.deleteSecret('generic-openai', state.bindingToken),
    ).resolves.toMatchObject({ configured: false });
    expect(records.size).toBe(0);
  });

  it('validates and canonicalizes Bedrock credentials before writing', async () => {
    const set = vi
      .fn()
      .mockResolvedValue({ providerId: 'bedrock', configured: true, updatedAt: 1 });
    const config = { providerId: 'bedrock' as const, region: 'us-west-2', modelId: 'profile-id' };
    const mutations = new ProviderMutationService(
      {
        get: () => config,
        credentialEpoch: () => 0,
        save: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
        advanceCredentialEpoch: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
      },
      {
        set,
        status: () => ({ providerId: 'bedrock', configured: false, updatedAt: null }),
        retainOnly: vi.fn(),
        purgeProvider: vi.fn(),
        unconfiguredStatus: () => ({ providerId: 'bedrock', configured: false, updatedAt: null }),
        activateEpoch: vi.fn(),
      },
      { credentialBinding: () => 'aws-sigv4:us-west-2', credentialPolicy: () => 'required' },
    );
    const token = (await mutations.secretStatus('bedrock')).bindingToken;

    await expect(mutations.setSecret('bedrock', token, 'not-json')).rejects.toMatchObject({
      code: 'INVALID_CONFIG',
    });
    expect(set).not.toHaveBeenCalled();

    await mutations.setSecret(
      'bedrock',
      token,
      '{"sessionToken":"session-token-example-123456","secretAccessKey":"valid-secret-access-key","accessKeyId":"AKIDEXAMPLE123456"}',
    );
    expect(vi.mocked(set)).toHaveBeenCalledWith(
      'bedrock',
      'aws-sigv4:us-west-2',
      JSON.stringify({
        accessKeyId: 'AKIDEXAMPLE123456',
        secretAccessKey: 'valid-secret-access-key',
        sessionToken: 'session-token-example-123456',
      }),
      1,
    );
  });

  it('advances the epoch and token whenever a credential is replaced or deleted', async () => {
    const test = harness();
    const initial = await test.mutations.secretStatus('generic-openai');

    const replaced = await test.mutations.setSecret(
      'generic-openai',
      initial.bindingToken,
      'replacement-key',
    );
    expect(replaced.bindingToken).not.toBe(initial.bindingToken);
    expect(test.calls).toContain('epoch:1');
    expect([...test.records.entries()]).toEqual([['generic-openai:1:A', 'replacement-key']]);

    const deleted = await test.mutations.deleteSecret('generic-openai', replaced.bindingToken);
    expect(deleted.bindingToken).not.toBe(replaced.bindingToken);
    expect(test.calls).toContain('epoch:2');
    expect(test.records.size).toBe(0);
  });

  it('activates valid epochs before one bulk startup credential reconciliation', async () => {
    const calls: string[] = [];
    const reconcileAll = vi.fn(() => {
      calls.push('reconcile');
      return Promise.resolve();
    });
    const mutations = new ProviderMutationService(
      {
        get: (providerId) => (providerId === 'generic-openai' ? endpointA : { providerId }),
        credentialEpoch: () => 3,
        save: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
        advanceCredentialEpoch: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
      },
      {
        set: vi.fn(),
        status: vi.fn(),
        retainOnly: vi.fn(),
        purgeProvider: vi.fn(),
        unconfiguredStatus: vi.fn(),
        activateEpoch: (providerId, epoch) => calls.push(`activate:${providerId}:${String(epoch)}`),
        reconcileAll,
      },
      {
        credentialBinding: (config) => {
          if (config.providerId !== 'generic-openai') throw new Error('incomplete draft');
          return 'A';
        },
        credentialPolicy: () => 'required',
      },
    );

    await mutations.reconcileAll();

    expect(reconcileAll).toHaveBeenCalledOnce();
    expect(reconcileAll).toHaveBeenCalledWith([
      { providerId: 'generic-openai', endpointBinding: 'A', credentialEpoch: 3 },
    ]);
    expect(calls).toEqual(['activate:generic-openai:3', 'reconcile']);
  });

  it('globally serializes provider config commits', async () => {
    const test = harness();
    const first = test.mutations.saveConfig(endpointB);
    const second = test.mutations.saveConfig({
      providerId: 'anthropic',
      modelId: 'claude-test',
    });
    await vi.waitFor(() => expect(test.calls).toContain('save:start:1'));
    expect(test.calls.some((call) => call.includes('anthropic'))).toBe(false);

    test.saveGate.resolve();
    await first;
    test.setGateSaves(false);
    await second;
    expect(test.calls.filter((call) => call.startsWith('save:start'))).toHaveLength(2);
  });
});

function deferred<Value = void>() {
  let resolvePromise!: (value?: Value | PromiseLike<Value>) => void;
  let rejectPromise!: (reason?: unknown) => void;
  const promise = new Promise<Value>((resolve, reject) => {
    resolvePromise = resolve as (value?: Value | PromiseLike<Value>) => void;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}
