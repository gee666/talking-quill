import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const [installer, bootstrap, packageJson, builderPatch] = await Promise.all([
  readFile(resolve(repositoryRoot, 'build', 'installer.nsh'), 'utf8'),
  readFile(resolve(repositoryRoot, 'installer', 'windows-bootstrap', 'src', 'windows.rs'), 'utf8'),
  readFile(resolve(repositoryRoot, 'package.json'), 'utf8'),
  readFile(resolve(repositoryRoot, 'patches', 'app-builder-lib@26.15.3.patch'), 'utf8'),
]);
const early = installer.slice(
  installer.indexOf('!macro TalkingQuillProtectedEarlyBootstrap'),
  installer.indexOf('!macroend', installer.indexOf('!macro TalkingQuillProtectedEarlyBootstrap')),
);
if (
  !early.includes('/TQPROTECTEDTEMP=') ||
  !early.includes('ReadEnvStr $R2 "TMP"') ||
  !early.includes('${If} $TEMP != $R1') ||
  /PowerShell|ExecShell|ExecWait|InitPluginsDir|nsExec::/iu.test(early)
) {
  throw new Error('NSIS early initialization must only validate the native protected TEMP handoff');
}
if (/windows-protected-bootstrap|TALKING_QUILL_PROTECTED_BOOTSTRAP_PAYLOAD/iu.test(installer)) {
  throw new Error('obsolete installer-side PowerShell bootstrap reference remains');
}
for (const required of [
  'ShellExecuteExW',
  'SEE_MASK_NOCLOSEPROCESS',
  'ConvertStringSecurityDescriptorToSecurityDescriptorW',
  'O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)',
  'FILE_ATTRIBUTE_REPARSE_POINT',
  'create_new(true)',
  'Sha256::digest',
  'WaitForSingleObject',
  'remove_file_and_directory',
]) {
  if (!bootstrap.includes(required))
    throw new Error(`native bootstrap check is missing ${required}`);
}
if (
  !builderPatch.includes('wrapTalkingQuillUninstaller') ||
  !builderPatch.includes('await packager.signIf(uninstallerPath);')
) {
  throw new Error('electron-builder must sign the inner and wrapped native uninstaller');
}
if (/nsis:bootstrap:(?:write|cleanup-stale)/u.test(packageJson)) {
  throw new Error('obsolete protected-bootstrap package commands remain');
}
for (const target of ['x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc']) {
  const result = spawnSync(
    'cargo',
    [
      'check',
      '--manifest-path',
      resolve(repositoryRoot, 'installer', 'windows-bootstrap', 'Cargo.toml'),
      '--target',
      target,
      '--locked',
    ],
    { cwd: repositoryRoot, stdio: 'inherit', windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`native bootstrap compile failed for ${target}`);
}
console.log('Native Windows bootstrap and plugin-free NSIS handoff checks passed.');
