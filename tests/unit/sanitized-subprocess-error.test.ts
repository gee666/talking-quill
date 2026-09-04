import { describe, expect, it } from 'vitest';
import { redactDiagnostic, subprocessFailure } from '../../scripts/sanitized-subprocess-error.mjs';

describe('sanitized subprocess diagnostics', () => {
  it('reports bounded process facts without paths or control characters', () => {
    const error = subprocessFailure('acceptance stage', {
      status: 7,
      signal: 'SIGTERM',
      error: { code: 'ENOENT', message: 'secret' },
      stderr: `failed C:\\protected\\request.der\n${'x'.repeat(2_000)}`,
    });
    expect(error.message).toContain('status=7, signal=SIGTERM, spawn=ENOENT');
    expect(error.message).toContain('stderr=<redacted>');
    expect(error.message).not.toContain('request.der');
    expect(error.message.length).toBeLessThan(1_200);
  });

  it('handles absent streams and never returns child-controlled diagnostics', () => {
    expect(subprocessFailure('stage', { status: null, signal: null }).message).toBe(
      'stage failed (status=none, signal=none, spawn=none)',
    );
    for (const diagnostic of [
      'secret-without-a-path',
      'open /home/runner/private/key.der failed',
      'open "C:\\Users\\Jane Doe\\private\\key.der" failed',
      '🔐'.repeat(2_000),
    ]) {
      expect(redactDiagnostic(diagnostic)).toBe('<redacted>');
    }
  });
});
