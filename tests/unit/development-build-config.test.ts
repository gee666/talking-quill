import { describe, expect, it } from 'vitest';
import configFactory from '../../app/electron.vite.config';

const createConfig = configFactory as unknown as (environment: { readonly mode: string }) => {
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
  });
});
