import { readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const renderer = resolve('app/src/renderer');

describe('Task 12 shipped-copy and AA audit', () => {
  it('retains the required free statement and excludes misleading product-tier copy', async () => {
    const source = await readTree(renderer);
    expect(source).toContain('Free to use — no account, no usage limits.');
    for (const prohibited of [
      /Pro tier/iu,
      /license key/iu,
      /daily allowance/iu,
      /usage counter/iu,
      /AnythingLLM (?:workspace|agent|product)/iu,
      /included cloud service/iu,
    ])
      expect(source).not.toMatch(prohibited);
    expect(source).toMatch(/cloud provider may charge|cloud providers may charge/iu);
  });

  it('keeps every normal text token at WCAG AA contrast on approved dark surfaces', () => {
    const surfaces = ['#0B0D10', '#12151A', '#181C22'];
    for (const foreground of ['#F4F7FA', '#9AA4B2']) {
      for (const background of surfaces)
        expect(contrast(foreground, background)).toBeGreaterThanOrEqual(4.5);
    }
  });
});

async function readTree(directory: string): Promise<string> {
  const chunks: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) chunks.push(await readTree(path));
    else if (/\.(?:ts|tsx|css)$/u.test(entry.name)) chunks.push(await readFile(path, 'utf8'));
  }
  return chunks.join('\n');
}
function contrast(a: string, b: string): number {
  const first = luminance(a);
  const second = luminance(b);
  const lighter = Math.max(first, second);
  const darker = Math.min(first, second);
  return (lighter + 0.05) / (darker + 0.05);
}
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255);
  const linear = channels.map((value) =>
    value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
  );
  return (linear[0] ?? 0) * 0.2126 + (linear[1] ?? 0) * 0.7152 + (linear[2] ?? 0) * 0.0722;
}
