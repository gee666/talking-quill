import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
const signing = require('../../build/mac-sign.cjs') as {
  isRustHelper(appPath: string, filePath: string): boolean;
  optionsForSignedFile(
    configuration: {
      app: string;
      optionsForFile?: (filePath: string) => Record<string, unknown>;
    },
    filePath: string,
  ): Record<string, unknown>;
};
const app = resolve('tmp/signing-fixture/Talking Quill.app');
const rustHelper = resolve(app, 'Contents/Resources/helper/talking-quill-helper');

describe('least-privilege macOS signing policy', () => {
  it('resolves the configured hook from electron-builder package working directory', () => {
    const builder = readFileSync('build/electron-builder.yml', 'utf8');
    const configuredPath = /^ {2}sign: (.+)$/mu.exec(builder)?.[1];
    expect(configuredPath).toBe('../build/mac-sign.cjs');
    expect(existsSync(resolve('app', configuredPath ?? 'missing'))).toBe(true);
  });

  it('recognizes only the exact bundled Rust helper path', () => {
    expect(signing.isRustHelper(app, rustHelper)).toBe(true);
    expect(signing.isRustHelper(app, `${rustHelper}-backup`)).toBe(false);
    expect(
      signing.isRustHelper(app, resolve(app, 'Contents/Frameworks/helper/talking-quill-helper')),
    ).toBe(false);
  });

  it('removes inherited JIT entitlements from the Rust helper but retains hardened runtime', () => {
    const options = signing.optionsForSignedFile(
      {
        app,
        optionsForFile: () => ({ entitlements: 'entitlements.mac.inherit.plist', timestamp: true }),
      },
      rustHelper,
    );
    expect(options).toEqual({ entitlements: [], hardenedRuntime: true, timestamp: true });
  });

  it('preserves electron-builder options for Electron helpers and other signed files', () => {
    const electronHelper = resolve(
      app,
      'Contents/Frameworks/Talking Quill Helper.app/Contents/MacOS/Talking Quill Helper',
    );
    const inherited = { entitlements: 'entitlements.mac.inherit.plist', hardenedRuntime: true };
    expect(
      signing.optionsForSignedFile({ app, optionsForFile: () => inherited }, electronHelper),
    ).toBe(inherited);
  });
});
