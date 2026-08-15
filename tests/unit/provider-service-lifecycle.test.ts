import { describe, expect, it, vi } from 'vitest';
import type {
  PreparedCompletionCloseReason,
  PreparedProviderCompletion,
  SmartProvider,
} from '../../app/src/main/providers/contracts';
import { ProviderError } from '../../app/src/main/providers/errors';
import type { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';

const config = { providerId: 'openai' as const, modelId: 'fixture-model' };
const credential = 'fixture-secret-value';

function createProvider(
  prepareCompletion?: SmartProvider['prepareCompletion'],
): SmartProvider & { readonly cleanTranscript: ReturnType<typeof vi.fn> } {
  return {
    id: 'openai',
    credentialPolicy: 'required',
    credentialBinding: () => 'fixture-binding',
    validate: () => Promise.resolve({ ok: true, destination: 'cloud', modelCount: 1 }),
    listModels: () => Promise.resolve([]),
    capabilities: () => 'unsupported',
    cleanTranscript: vi.fn(() => Promise.resolve('ordinary fallback must not run')),
    classifyDestination: () => Promise.resolve('cloud'),
    ...(prepareCompletion === undefined ? {} : { prepareCompletion: vi.fn(prepareCompletion) }),
  };
}

function createService(
  provider: SmartProvider,
  options: { readonly operationTimeoutMs?: number } = {},
): ProviderService {
  const registry = {
    get: () => provider,
    catalog: () => [],
  } as unknown as ProviderRegistry;
  return new ProviderService(registry, { getCredential: () => credential }, options);
}

function preparedFixture(options: { readonly closeImmediately?: boolean } = {}) {
  const closed = deferred<undefined>();
  const reasons: PreparedCompletionCloseReason[] = [];
  const complete = vi.fn<PreparedProviderCompletion['complete']>(() => Promise.resolve('cleaned'));
  const requestClose = vi.fn<PreparedProviderCompletion['requestClose']>((reason) => {
    reasons.push(reason);
    if (options.closeImmediately === true) closed.resolve(undefined);
  });
  const prepared: PreparedProviderCompletion = { complete, requestClose, closed: closed.promise };
  return { prepared, complete, requestClose, closed, reasons };
}

describe('ProviderService prepared completion lifecycle', () => {
  it('invokes prepared capabilities with their provider receiver', async () => {
    let receivedProvider = false;
    const provider = createProvider(function (this: SmartProvider) {
      receivedProvider = this.id === 'openai' && this.credentialPolicy === 'required';
      return Promise.resolve(null);
    });
    const service = createService(provider);

    await expect(
      service.prepareCompletion(config, new AbortController().signal),
    ).resolves.toBeNull();
    expect(receivedProvider).toBe(true);
  });

  it('returns null for providers without the optional capability without changing print completion', async () => {
    const provider = createProvider();
    const service = createService(provider);

    await expect(
      service.prepareCompletion(config, new AbortController().signal),
    ).resolves.toBeNull();
    await expect(
      service.cleanTranscript(config, { input: 'raw' }, new AbortController().signal),
    ).resolves.toBe('ordinary fallback must not run');
    expect(provider.cleanTranscript).toHaveBeenCalledOnce();
  });

  it('normalizes and consumes exactly one request, closes idempotently, and exposes retirement', async () => {
    const raw = preparedFixture({ closeImmediately: true });
    const provider = createProvider(() => Promise.resolve(raw.prepared));
    const service = createService(provider);
    const lease = await service.prepareCompletion(config, new AbortController().signal);
    if (lease === null) throw new Error('expected prepared lease');

    await expect(
      lease.complete({ input: 'raw transcript' }, new AbortController().signal),
    ).resolves.toBe('cleaned');
    await expect(lease.closed).resolves.toBeUndefined();
    lease.requestClose('shutdown');
    await expect(
      lease.complete({ input: 'second prompt' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG', fallbackEligible: false });

    expect(raw.complete).toHaveBeenCalledOnce();
    expect(raw.complete.mock.calls[0]?.[0]).toEqual({
      input: 'raw transcript',
      temperature: 0.2,
    });
    expect(raw.reasons).toEqual(['completed']);
    await expect(service.drain()).resolves.toBeUndefined();
  });

  it('marks only known pre-dispatch failures as fallback eligible and never invokes print fallback', async () => {
    const oversized = preparedFixture({ closeImmediately: true });
    const provider = createProvider(() => Promise.resolve(oversized.prepared));
    const service = createService(provider);
    const oversizedLease = await service.prepareCompletion(config, new AbortController().signal);
    if (oversizedLease === null) throw new Error('expected prepared lease');

    await expect(
      oversizedLease.complete({ input: '😀'.repeat(122_881) }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'REQUEST_TOO_LARGE', fallbackEligible: true });
    expect(oversized.complete).not.toHaveBeenCalled();

    if (provider.prepareCompletion === undefined) throw new Error('expected capability');
    const prepareCompletion = vi.spyOn(provider, 'prepareCompletion');
    const beforeDispatch = preparedFixture({ closeImmediately: true });
    beforeDispatch.complete.mockRejectedValueOnce(
      new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true }),
    );
    // The provider capability is the authority for failures before its prompt write boundary.
    prepareCompletion.mockResolvedValueOnce(beforeDispatch.prepared);
    const beforeLease = await service.prepareCompletion(config, new AbortController().signal);
    if (beforeLease === null) throw new Error('expected prepared lease');
    await expect(
      beforeLease.complete({ input: 'safe retry' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED', fallbackEligible: true });

    const ambiguous = preparedFixture({ closeImmediately: true });
    ambiguous.complete.mockRejectedValueOnce(new ProviderError('PI_LAUNCH_FAILED'));
    prepareCompletion.mockResolvedValueOnce(ambiguous.prepared);
    const ambiguousLease = await service.prepareCompletion(config, new AbortController().signal);
    if (ambiguousLease === null) throw new Error('expected prepared lease');
    await expect(
      ambiguousLease.complete({ input: 'one prompt only' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED', fallbackEligible: false });
    await expect(
      ambiguousLease.complete({ input: 'must not retry' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_CONFIG', fallbackEligible: false });

    expect(ambiguous.complete).toHaveBeenCalledOnce();
    expect(provider.cleanTranscript).not.toHaveBeenCalled();
    await service.drain();
  });

  it('rejects a capability that was already closed before ownership transfer', async () => {
    const raw = preparedFixture();
    raw.closed.resolve(undefined);
    const provider = createProvider(() => Promise.resolve(raw.prepared));
    const service = createService(provider);

    await expect(
      service.prepareCompletion(config, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
    expect(raw.complete).not.toHaveBeenCalled();
    await service.drain();
  });

  it('never dispatches through a lease that closed before use', async () => {
    const raw = preparedFixture();
    const provider = createProvider(() => Promise.resolve(raw.prepared));
    const service = createService(provider);
    const lease = await service.prepareCompletion(config, new AbortController().signal);
    if (lease === null) throw new Error('expected prepared lease');

    raw.closed.resolve(undefined);
    await lease.closed;
    await expect(
      lease.complete({ input: 'too late' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE', fallbackEligible: true });
    expect(raw.complete).not.toHaveBeenCalled();
  });

  it('applies credential-echo validation to prepared output after dispatch', async () => {
    const raw = preparedFixture({ closeImmediately: true });
    raw.complete.mockResolvedValueOnce(`unsafe ${credential}`);
    const provider = createProvider(() => Promise.resolve(raw.prepared));
    const service = createService(provider);
    const lease = await service.prepareCompletion(config, new AbortController().signal);
    if (lease === null) throw new Error('expected prepared lease');

    await expect(
      lease.complete({ input: 'raw transcript' }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE', fallbackEligible: false });
    expect(raw.reasons).toEqual(['failed']);
    expect(provider.cleanTranscript).not.toHaveBeenCalled();
  });

  it('retires a lease that arrives after cancellation and keeps drain pending until late close', async () => {
    const preparation = deferred<PreparedProviderCompletion | null>();
    const raw = preparedFixture();
    const provider = createProvider(() => preparation.promise);
    const prepareCompletion = vi.spyOn(provider, 'prepareCompletion');
    const service = createService(provider);
    const pending = service.prepareCompletion(config, new AbortController().signal);
    await vi.waitFor(() => expect(prepareCompletion).toHaveBeenCalledOnce());

    service.dispose();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED', fallbackEligible: true });
    preparation.resolve(raw.prepared);
    await vi.waitFor(() => expect(raw.reasons).toEqual(['shutdown']));

    let drained = false;
    const drain = service.drain().then(() => {
      drained = true;
    });
    await Promise.resolve();
    expect(drained).toBe(false);
    raw.closed.resolve(undefined);
    await drain;
    expect(drained).toBe(true);
    await expect(
      service.prepareCompletion(config, new AbortController().signal),
    ).rejects.toMatchObject({
      code: 'UNAVAILABLE',
    });
  });

  it('stops admission separately, aborts active leases, and reports failed retirement after settling', async () => {
    const raw = preparedFixture();
    const provider = createProvider(() => Promise.resolve(raw.prepared));
    const service = createService(provider);
    const lease = await service.prepareCompletion(config, new AbortController().signal);
    if (lease === null) throw new Error('expected prepared lease');

    service.stopAccepting();
    await expect(
      service.prepareCompletion(config, new AbortController().signal),
    ).rejects.toMatchObject({
      code: 'UNAVAILABLE',
    });
    service.abortAll();
    service.abortAll();
    expect(raw.reasons).toEqual(['cancelled']);

    const drain = service.drain();
    raw.closed.reject(new ProviderError('PI_LAUNCH_FAILED'));
    await expect(lease.closed).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED' });
    await expect(drain).rejects.toMatchObject({ code: 'PI_LAUNCH_FAILED' });
    expect(raw.requestClose).toHaveBeenCalledOnce();
  });

  it('cancels non-cooperative ordinary calls while retaining them for bounded drain', async () => {
    const provider = createProvider();
    const underlying = deferred<string>();
    provider.cleanTranscript.mockImplementationOnce(() => underlying.promise);
    const service = createService(provider);
    const caller = new AbortController();
    const completion = service.cleanTranscript(config, { input: 'raw' }, caller.signal);
    await vi.waitFor(() => expect(provider.cleanTranscript).toHaveBeenCalledOnce());

    caller.abort();
    await expect(completion).rejects.toMatchObject({ code: 'CANCELLED' });
    service.stopAccepting();
    let drained = false;
    const drain = service.drain().then(() => {
      drained = true;
    });
    await Promise.resolve();
    expect(drained).toBe(false);

    underlying.resolve('late output is ignored');
    await drain;
    expect(drained).toBe(true);
  });

  it('enforces the prepared lease deadline without dispatching a late prompt', async () => {
    vi.useFakeTimers();
    try {
      const raw = preparedFixture();
      const provider = createProvider(() => Promise.resolve(raw.prepared));
      const service = createService(provider, { operationTimeoutMs: 20 });
      const lease = await service.prepareCompletion(config, new AbortController().signal);
      if (lease === null) throw new Error('expected prepared lease');

      await vi.advanceTimersByTimeAsync(20);
      expect(raw.reasons).toEqual(['timeout']);
      await expect(
        lease.complete({ input: 'too late' }, new AbortController().signal),
      ).rejects.toMatchObject({ code: 'TIMEOUT', fallbackEligible: true });
      expect(raw.complete).not.toHaveBeenCalled();
      raw.closed.resolve(undefined);
      await service.drain();
    } finally {
      vi.useRealTimers();
    }
  });
});

function deferred<Result>() {
  let resolve!: (result: Result) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Result>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
