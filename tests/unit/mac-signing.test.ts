import { existsSync, readFileSync } from 'node:fs';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
const signing = require('../../build/mac-sign.cjs') as {
  certificateBytes(link: string): Promise<Buffer>;
  containedRepositoryPath(input: string): string;
  findMacApplication(packageRoot: string): string;
  isRustHelper(appPath: string, filePath: string): boolean;
  optionsForSignedFile(
    configuration: {
      app: string;
      optionsForFile?: (filePath: string) => Record<string, unknown>;
    },
    filePath: string,
  ): Record<string, unknown>;
  verifyEntitlements(entitlements: string): void;
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

  it('preserves reviewed electron-builder options for Electron helpers', () => {
    const electronHelper = resolve(
      app,
      'Contents/Frameworks/Talking Quill Helper.app/Contents/MacOS/Talking Quill Helper',
    );
    const inherited = { entitlements: 'entitlements.mac.inherit.plist', hardenedRuntime: true };
    expect(
      signing.optionsForSignedFile({ app, optionsForFile: () => inherited }, electronHelper),
    ).toBe(inherited);
  });

  it('binds entitlement bytes to the protected signer and rejects changed policy files', async () => {
    const fixture = resolve('tmp/signing-fixture/changed/entitlements.mac.plist');
    await mkdir(resolve(fixture, '..'), { recursive: true });
    await writeFile(fixture, '<plist><dict/></plist>');
    try {
      expect(() => signing.verifyEntitlements('entitlements.mac.plist')).not.toThrow();
      expect(() => signing.verifyEntitlements(fixture)).toThrow(
        'Reviewed entitlement policy changed',
      );
      expect(() => signing.verifyEntitlements('unreviewed.plist')).toThrow(
        'Unreviewed entitlement file',
      );
    } finally {
      await rm(resolve('tmp/signing-fixture/changed'), { recursive: true, force: true });
    }
  });

  it('validates contained prebuilt roots and supported certificate encodings', async () => {
    const root = resolve('tmp/signing-fixture/prebuilt');
    const application = resolve(root, 'Configured Product.app');
    await mkdir(application, { recursive: true });
    try {
      expect(signing.findMacApplication(root)).toBe(application);
      expect(signing.containedRepositoryPath('release/win-unpacked')).toBe(
        resolve('release/win-unpacked'),
      );
      expect(() => signing.containedRepositoryPath('../outside')).toThrow('contained repository');
      const certificate = Buffer.from('certificate fixture');
      await expect(signing.certificateBytes(certificate.toString('base64'))).resolves.toEqual(
        certificate,
      );
      await expect(
        signing.certificateBytes(
          `data:application/x-pkcs12;base64,${certificate.toString('base64')}`,
        ),
      ).resolves.toEqual(certificate);
      await expect(signing.certificateBytes('not-valid-base64%')).rejects.toThrow('CSC_LINK');
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
