import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { canonicalJson } from './release-manifest.mjs';

// Inventory of automated evidence only, not a replacement for the separately
// signed real-reboot or protected installed-acceptance evidence formats.
export async function windowsReleaseValidation(directory, verify = false) {
  const evidence = [];
  for (const architecture of ['x64', 'arm64']) {
    for (const prefix of [
      'windows-installer-ui-smoke',
      'windows-installer-success-fresh',
      'windows-local-migration',
      'windows-terminal-fault-candidate',
    ]) {
      const name = `${prefix}-${architecture}.json`;
      const bytes = await readFile(resolve(directory, name));
      JSON.parse(bytes.toString('utf8'));
      evidence.push({ name, sha256: createHash('sha256').update(bytes).digest('hex') });
    }
  }
  const report = {
    schemaVersion: 1,
    scope: 'automated-fresh-release',
    realReboot: 'not-collected',
    protectedInstalledAcceptance: 'not-collected',
    evidence,
  };
  const path = resolve(directory, 'windows-release-validation.json');
  if (verify) {
    if (canonicalJson(JSON.parse(await readFile(path, 'utf8'))) !== canonicalJson(report))
      throw new Error('Automated release validation inventory mismatch');
  } else {
    await writeFile(path, `${canonicalJson(report)}\n`);
  }
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (!process.argv[2] || process.argv.slice(3).some((arg) => arg !== '--verify'))
    throw new Error('Usage: windows-release-validation.mjs DIRECTORY [--verify]');
  await windowsReleaseValidation(process.argv[2], process.argv.includes('--verify'));
}
