import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  boundedDiagnosticCapture,
  redactLifecycleDiagnostic,
} from '../../scripts/windows-package-lifecycle.mjs';

describe('Windows lifecycle diagnostics', () => {
  it('bounds retained bytes while continuing to count drained output', () => {
    const capture = boundedDiagnosticCapture(5);
    capture.append(Buffer.from('abc'));
    capture.append(Buffer.from('defgh'));
    capture.append(Buffer.alloc(1_000_000));
    expect(capture.snapshot()).toEqual({ text: 'abcde', bytes: 1_000_008, truncated: true });
  });

  it('redacts profile paths, credentials and launch correlation without hiding errors', () => {
    const text = redactLifecycleDiagnostic(
      `Startup failed C:\\Users\\runner\\profile token=abc password=def api_key=ghi Bearer xyz correlation=${'a'.repeat(64)} S-1-5-21-123`,
      ['C:\\Users\\runner', 'C:\\Users\\runner\\profile'],
    );
    expect(text).toContain('Startup failed <path>');
    for (const secret of ['abc', 'def', 'ghi', 'xyz', 'a'.repeat(64), 'S-1-5-21-123'])
      expect(text).not.toContain(secret);
  });

  it('exports only bounded known logs and keeps raw profiles out of artifacts', () => {
    const lifecycle = readFileSync('scripts/windows-package-lifecycle.mjs', 'utf8');
    const hosted = readFileSync('scripts/windows-hosted-runtime-smoke.mjs', 'utf8');
    expect(hosted).toContain("resolve(output, 'diagnostics')");
    expect(hosted).toContain('windows-production-startup-smoke.ps1');
    expect(hosted).not.toContain("resolve(output, 'runtime-profiles')");
    expect(lifecycle).toContain("['ignore', 'pipe', 'pipe']");
    expect(lifecycle).toContain("'readiness-error.txt'");
    expect(lifecycle).toContain('beforeCleanup');
    expect(lifecycle).toContain('Buffer.alloc(DIAGNOSTIC_BYTE_LIMIT)');
    expect(lifecycle).toContain('diagnostic.jsonl');
    expect(lifecycle).toContain('45_000, `${label} readiness timed out`');
  });
});
