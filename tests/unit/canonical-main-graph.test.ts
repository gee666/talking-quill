import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { transformWindowsInstalledAcceptanceSource } from '../../app/windows-installed-acceptance-overlay';
import {
  verifyCanonicalHelperTransportSources,
  verifyCanonicalMainGraph,
} from '../../scripts/canonical-main-graph.mjs';

const sources = {
  application: 'app/src/main/app/application.ts',
  bootstrap: 'app/src/main/bootstrap.ts',
  diagnostic: 'app/src/main/security/diagnostic-logger.ts',
  helperClient: 'app/src/main/helper/helper-client.ts',
  helperChannel: 'app/src/main/helper/helper-rpc-channel.ts',
} as const;

const fixtureRoot = resolve('tmp/canonical-main-graph-test');

afterEach(() => rm(fixtureRoot, { recursive: true, force: true }));

describe('canonical main import graph', () => {
  it('contains no acceptance dispatch, fault injection, or arbitrary helper RPC', async () => {
    const graph = await verifyCanonicalMainGraph();
    expect(graph).toContain('app/src/main/app/application.ts');
    expect(graph).toContain('app/src/main/helper/helper-client.ts');
    expect(graph.some((path) => path.includes('/acceptance/'))).toBe(false);
  });

  it.each(["void import('./main/acceptance/probe')", "require('./main/acceptance/probe')"])(
    'follows executable module loads: %s',
    async (moduleLoad) => {
      await mkdir(resolve(fixtureRoot, 'main/acceptance'), { recursive: true });
      await writeFile(resolve(fixtureRoot, 'index.ts'), `${moduleLoad};\n`, 'utf8');
      await writeFile(resolve(fixtureRoot, 'main/acceptance/probe.ts'), 'export {};\n', 'utf8');
      await expect(verifyCanonicalMainGraph(resolve(fixtureRoot, 'index.ts'))).rejects.toThrow(
        'acceptance-only module is reachable',
      );
    },
  );

  it('rejects alternate public paths to the helper RPC frame encoder', async () => {
    const [helperClient, helperChannel] = await Promise.all([
      readFile(sources.helperClient, 'utf8'),
      readFile(sources.helperChannel, 'utf8'),
    ]);
    const mutated = helperChannel.replace(
      '  close(session: HelperRpcSession',
      `  sendAny(method: unknown, session: HelperRpcSession) {
    return this.request(session, method as never, {} as never, {} as never);
  }

  close(session: HelperRpcSession`,
    );
    expect(() => verifyCanonicalHelperTransportSources(helperClient, mutated)).toThrow(
      'one public HelperMethod-typed request method',
    );
  });

  it('adds concrete installed acceptance behavior only in the acceptance overlay', async () => {
    const transformed: Record<keyof typeof sources, string> = {
      application: '',
      bootstrap: '',
      diagnostic: '',
      helperClient: '',
      helperChannel: '',
    };
    for (const [name, path] of Object.entries(sources) as [keyof typeof sources, string][]) {
      transformed[name] = transformWindowsInstalledAcceptanceSource(
        name,
        await readFile(path, 'utf8'),
      );
    }
    expect(transformed.application).toContain('runInstalledAcceptance(');
    expect(transformed.application).toContain('this.#helper as InstalledAcceptanceHelper');
    expect(transformed.bootstrap).toContain('await application.runInstalledAcceptance(');
    expect(transformed.bootstrap).toContain('await application.stop()');
    expect(transformed.diagnostic).toContain('acceptance-injected-write-failure');
    expect(transformed.helperClient).toContain('requestAcceptance(method: string');
    expect(transformed.helperChannel).toContain('requestAcceptance(');
    expect(transformed.helperChannel).toContain('pending.resultSchema.safeParse');
  });

  it('fails closed when an overlay source anchor drifts', () => {
    expect(() =>
      transformWindowsInstalledAcceptanceSource('diagnostic', 'class Changed {}'),
    ).toThrow('source anchor is missing');
  });
});
