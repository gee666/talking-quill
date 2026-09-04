import { spawnSync } from 'node:child_process';
import { describe, expect, it } from 'vitest';
import { readAcceptanceSecretPaths } from '../../scripts/acceptance-secret-path-frame.mjs';

describe('acceptance secret-path stdin frame', () => {
  it('reads a newline-terminated frame from the numeric stdin descriptor', () => {
    const frame = { requestPrivateKeyPath: 'C:\\protected\\request.der' };
    const result = spawnSync(
      process.execPath,
      [
        '--input-type=module',
        '--eval',
        "import { readAcceptanceSecretPaths } from './scripts/acceptance-secret-path-frame.mjs'; process.stdout.write(JSON.stringify(readAcceptanceSecretPaths()));",
      ],
      {
        cwd: process.cwd(),
        encoding: 'utf8',
        input: `${JSON.stringify(frame)}\n`,
      },
    );
    expect(result).toMatchObject({ status: 0, stderr: '' });
    expect(JSON.parse(result.stdout)).toEqual(frame);
  });

  it('rejects empty, unterminated, and oversized frames', () => {
    for (const bytes of [Buffer.alloc(0), Buffer.from('{}'), Buffer.alloc(16 * 1024 + 1, 0x0a)]) {
      expect(() => readAcceptanceSecretPaths(() => bytes)).toThrow(
        'Acceptance sealing secret-path frame is invalid',
      );
    }
  });
});
