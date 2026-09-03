import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import configFactory from '../../app/electron.vite.config';

const createConfig = configFactory as unknown as (environment: { readonly mode: string }) => {
  readonly main?: {
    readonly plugins?: readonly { readonly name?: string }[];
    readonly define?: Readonly<Record<string, string>>;
    readonly build?: { readonly rollupOptions?: { readonly input?: Record<string, string> } };
  };
  readonly preload?: { readonly build?: { readonly emptyOutDir?: boolean } };
  readonly renderer?: { readonly plugins?: readonly unknown[] };
};

const entry = (mode: string) => createConfig({ mode }).main?.build?.rollupOptions?.input?.index;

afterEach(() => {
  delete process.env.TALKING_QUILL_PACKAGE_VARIANT;
  delete process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL;
});

describe('main entry build configuration', () => {
  it('uses the development entry outside production and preserves standalone preloads', () => {
    const config = createConfig({ mode: 'development' });
    expect(entry('development')).toMatch(/entries[\\/]development\.ts$/u);
    expect(config.renderer?.plugins).toEqual([]);
    expect(config.preload?.build?.emptyOutDir).toBe(false);
  });

  it('keeps isolated source profiles on the single-instance lock path', () => {
    const developmentEntry = readFileSync(resolve('app/src/main/entries/development.ts'), 'utf8');
    expect(developmentEntry).toContain('{ userDataPath: resolve(profile) }');
    expect(developmentEntry).not.toContain('isolatedInstance: true');
  });

  it('builds only the canonical main entry for production', () => {
    const config = createConfig({ mode: 'production' });
    expect(entry('production')).toMatch(/main[\\/]index\.ts$/u);
    expect(config.renderer?.plugins).toHaveLength(1);
    expect(config.preload?.build?.emptyOutDir).toBeUndefined();
    expect(config.main?.define).not.toHaveProperty('__TALKING_QUILL_ACCEPTANCE_BUILD__');
    expect(config.main?.plugins).toEqual([]);
  });

  it('keeps the directory-test variant on the canonical production entry', () => {
    process.env.TALKING_QUILL_PACKAGE_VARIANT = 'directory-test';
    const config = createConfig({ mode: 'production' });
    expect(config.main?.build?.rollupOptions?.input?.index).toMatch(/main[\\/]index\.ts$/u);
    expect(config.main?.plugins).toEqual([]);
  });

  it('selects acceptance only through the explicit noncanonical package variant', () => {
    process.env.TALKING_QUILL_PACKAGE_VARIANT = 'installed-acceptance';
    process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL = 'public_key';
    const config = createConfig({ mode: 'production' });
    expect(config.main?.build?.rollupOptions?.input?.index).toMatch(
      /entries[\\/]windows-installed-acceptance\.ts$/u,
    );
    expect(config.main?.plugins?.map(({ name }) => name)).toEqual([
      'talking-quill-windows-installed-acceptance-overlay',
    ]);
  });

  it('selects the packaged-test entry only through its explicit test build variant', () => {
    process.env.TALKING_QUILL_PACKAGE_VARIANT = 'packaged-test';
    expect(entry('test')).toMatch(/entries[\\/]packaged-test\.ts$/u);
    expect(() => entry('production')).toThrow('requires test build mode');
  });
});
