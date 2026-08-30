import { describe, expect, it } from 'vitest';
import configFactory from '../../app/electron.vite.config';

const createConfig = configFactory as unknown as (environment: { readonly mode: string }) => {
  readonly main?: { readonly define?: Readonly<Record<string, string>> };
  readonly preload?: { readonly build?: { readonly emptyOutDir?: boolean } };
  readonly renderer?: { readonly plugins?: readonly unknown[] };
};

describe('development renderer build configuration', () => {
  it('keeps strict CSP compatible with the dev renderer and preserves standalone preloads', () => {
    const config = createConfig({ mode: 'development' });

    expect(config.renderer?.plugins).toEqual([]);
    expect(config.preload?.build?.emptyOutDir).toBe(false);
  });

  it('keeps the canonical preload output for production builds', () => {
    const config = createConfig({ mode: 'production' });

    expect(config.renderer?.plugins).toHaveLength(1);
    expect(config.preload?.build?.emptyOutDir).toBeUndefined();
    expect(config.main?.define?.__TALKING_QUILL_ACCEPTANCE_BUILD__).toBe('false');
  });

  it('enables acceptance only at compile time with both authorization inputs', () => {
    process.env.TALKING_QUILL_ACCEPTANCE_BUILD = '1';
    process.env.TALKING_QUILL_ACCEPTANCE_BUILD_MANIFEST_BASE64URL = 'manifest';
    process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL = 'public_key';
    try {
      const config = createConfig({ mode: 'production' });
      expect(config.main?.define?.__TALKING_QUILL_ACCEPTANCE_BUILD__).toBe('true');
    } finally {
      delete process.env.TALKING_QUILL_ACCEPTANCE_BUILD;
      delete process.env.TALKING_QUILL_ACCEPTANCE_BUILD_MANIFEST_BASE64URL;
      delete process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL;
    }
  });
});
