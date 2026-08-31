import { access, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const required = [
  'installer/windows-setup/Cargo.toml',
  'installer/windows-setup/src/main.rs',
  'installer/windows-setup/src/package.rs',
  'installer/windows-setup/src/windows.rs',
  'scripts/pack-windows-native.mjs',
  'scripts/tqpkg2.mjs',
];
for (const path of required) await access(resolve(root, path));
const [builder, application, packer, setup] = await Promise.all([
  readFile(resolve(root, 'build/electron-builder.yml'), 'utf8'),
  readFile(resolve(root, 'app/package.json'), 'utf8'),
  readFile(resolve(root, 'scripts/pack-windows-native.mjs'), 'utf8'),
  readFile(resolve(root, 'installer/windows-setup/src/windows.rs'), 'utf8'),
]);
if (!builder.includes('- target: dir') || !application.includes('pack-windows-native.mjs')) {
  throw new Error('Windows packaging does not use the native setup pipeline');
}
for (const evidence of ['TQPKG2', 'canonicalJson', 'zstdCompressSync', 'blockMapSize']) {
  if (!packer.includes(evidence))
    throw new Error(`native package evidence is missing: ${evidence}`);
}
for (const evidence of ['FILE_FLAG_OVERLAPPED', 'NtSuspendProcess', 'diffie_hellman', 'Hmac', 'TokenIntegrityLevel', 'OpenSCManagerW', 'ITaskService', 'FOLDERID_RoamingAppData', 'FILE_FLAG_OPEN_REPARSE_POINT']) {
  if (!setup.includes(evidence)) throw new Error(`native setup security mechanism is missing: ${evidence}`);
}
for (const forbidden of ['powershell', 'cmd.exe', 'wscript', 'cscript']) {
  if (setup.toLowerCase().includes(forbidden))
    throw new Error(`native setup invokes an interpreter: ${forbidden}`);
}
console.log('Native Windows installer policy passed');
