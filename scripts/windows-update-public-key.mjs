import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export function publicKeySha256FromSec1Hex(text) {
  const normalized = text.trim();
  if (!/^04[0-9a-f]{128}$/iu.test(normalized)) {
    throw new Error('Windows update public key must be one uncompressed P-256 SEC1 value');
  }
  return createHash('sha256').update(Buffer.from(normalized, 'hex')).digest('hex');
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const [path, ...extra] = process.argv.slice(2);
  if (!path || extra.length !== 0) {
    throw new Error('Usage: windows-update-public-key <SEC1-hex-path>');
  }
  console.log(publicKeySha256FromSec1Hex(readFileSync(path, 'utf8')));
}
