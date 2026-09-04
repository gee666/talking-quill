import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, existsSync } from 'node:fs';
import {
  copyFile,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  rm,
  rmdir,
  writeFile,
} from 'node:fs/promises';
import { basename, dirname, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { subprocessFailure } from './sanitized-subprocess-error.mjs';
import { launchVerifiedChildSync } from './windows-verified-child-launcher.mjs';

const HEX = /^[0-9a-f]{64}$/u;
const PURPOSE = 'talking-quill/installed-acceptance-native-publication/v1';
const LAYOUTS = Object.freeze({
  producer: Object.freeze({
    signer: 'talking-quill-acceptance-signer.exe',
    broker: 'talking-quill-windows-acceptance-broker.exe',
    bootstrap: 'talking-quill-helper.exe',
  }),
  bundle: Object.freeze({
    broker: 'acceptance-broker.exe',
    bootstrap: 'acceptance-bootstrap.exe',
    launcher: 'trusted-launcher.exe',
    sender: 'synthetic-sender.exe',
  }),
});

export async function publishAcceptanceNative({
  buildId,
  sourceRoot,
  outputRoot,
  programData = process.env.ProgramData ?? 'C:/ProgramData',
  layout = 'producer',
}) {
  if (process.platform !== 'win32') throw new Error('Native publication requires Windows');
  if (!HEX.test(buildId ?? ''))
    throw new Error('Protected native publication build identity is invalid');
  const files = LAYOUTS[layout];
  if (files === undefined) throw new Error('Native publication layout is invalid');
  const nativeBase = resolve(programData, 'Talking Quill Acceptance Native');
  const nativeRoot = resolve(nativeBase, buildId);
  const removeNativeBase = !existsSync(nativeBase);
  const cleanupLauncherPath = resolve(outputRoot, 'native-publication-cleanup-helper.exe');
  let nativeRootCreated = false;
  let cleanupLauncherCreated = false;
  let initializedRoot = false;
  let protectedRoot = false;
  try {
    await copyFile(
      resolve(sourceRoot, files.bootstrap),
      cleanupLauncherPath,
      constants.COPYFILE_EXCL,
    );
    cleanupLauncherCreated = true;
    await mkdir(nativeBase, { recursive: true });
    await mkdir(nativeRoot, { recursive: false });
    nativeRootCreated = true;
    initializeNativeRoot(nativeRoot);
    initializedRoot = true;
    for (const name of Object.values(files)) {
      await copyFile(resolve(sourceRoot, name), resolve(nativeRoot, name), constants.COPYFILE_EXCL);
    }
    protectNativeRoot(nativeRoot);
    protectedRoot = true;
    const { userSid, rootIdentity, inventory } = await verifyNativeRoot(nativeRoot);
    const cleanupLauncher = await fileIdentity(cleanupLauncherPath);
    const descriptorPath = resolve(outputRoot, 'native-publication-cleanup.json');
    const descriptor = {
      schemaVersion: 1,
      purpose: PURPOSE,
      buildId,
      layout,
      removeNativeBase,
      nativeBase,
      nativeRoot,
      userSid,
      rootIdentity,
      inventory,
      cleanupLauncher: { path: cleanupLauncherPath, ...cleanupLauncher },
      descriptorPath,
    };
    await writeFile(descriptorPath, `${JSON.stringify(descriptor)}\n`, {
      flag: 'wx',
      mode: 0o600,
    });
    return Object.freeze(descriptor);
  } catch (error) {
    try {
      if (nativeRootCreated) {
        await removeFailedPublication(
          nativeBase,
          nativeRoot,
          cleanupLauncherPath,
          layout,
          initializedRoot,
          protectedRoot,
        );
      }
      if (cleanupLauncherCreated) await rm(cleanupLauncherPath, { force: false });
      if (removeNativeBase) await removeEmptyBase(nativeBase);
    } catch (cleanupError) {
      throw new AggregateError([error, cleanupError], 'Native publication and cleanup failed');
    }
    throw error;
  }
}

export async function cleanupAcceptanceNativeDescriptor(descriptorPath, options) {
  const absolute = resolve(descriptorPath);
  const descriptor = JSON.parse(await readFile(absolute, 'utf8'));
  if (resolve(descriptor?.descriptorPath ?? '') !== absolute) {
    throw new Error('Native publication descriptor path is not canonical');
  }
  return cleanupAcceptanceNative(descriptor, options);
}

export async function cleanupAcceptanceNative(
  descriptor,
  { programData = process.env.ProgramData ?? 'C:/ProgramData' } = {},
) {
  const checked = validateDescriptor(descriptor);
  if (resolve(checked.nativeBase) !== resolve(programData, 'Talking Quill Acceptance Native')) {
    throw new Error('Native publication cleanup base is unauthorized');
  }
  if (!existsSync(checked.nativeRoot)) {
    throw new Error('Native publication root is absent before authenticated cleanup');
  }
  const verified = await verifyNativeRoot(checked.nativeRoot, checked.userSid, checked.layout);
  if (
    verified.rootIdentity !== checked.rootIdentity ||
    JSON.stringify(verified.inventory) !== JSON.stringify(checked.inventory)
  ) {
    throw new Error('Native publication identity or inventory changed before cleanup');
  }
  const launcher = await fileIdentity(checked.cleanupLauncher.path);
  if (
    launcher.sha256 !== checked.cleanupLauncher.sha256 ||
    launcher.bytes !== checked.cleanupLauncher.bytes ||
    launcher.identity !== checked.cleanupLauncher.identity
  ) {
    throw new Error('Native publication cleanup launcher changed');
  }
  await removeExactPublication(checked, launcher);
  if (existsSync(checked.nativeRoot)) throw new Error('Native publication cleanup left residue');
  await removeCleanupFile(checked.cleanupLauncher.path);
  await removeCleanupFile(checked.descriptorPath);
  if (checked.removeNativeBase) await removeEmptyBase(checked.nativeBase);
  return Object.freeze({ result: 'deleted' });
}

export function validateDescriptor(value) {
  if (
    value?.schemaVersion !== 1 ||
    value?.purpose !== PURPOSE ||
    !HEX.test(value?.buildId ?? '') ||
    LAYOUTS[value?.layout] === undefined ||
    typeof value?.removeNativeBase !== 'boolean' ||
    !isAbsolute(value?.nativeBase ?? '') ||
    !isAbsolute(value?.nativeRoot ?? '') ||
    resolve(value.nativeRoot) !== resolve(value.nativeBase, value.buildId) ||
    !/^S-[0-9-]+$/u.test(value?.userSid ?? '') ||
    !validIdentity(value?.rootIdentity) ||
    !Array.isArray(value?.inventory) ||
    value.inventory.length !== Object.keys(LAYOUTS[value.layout] ?? {}).length ||
    !isAbsolute(value?.cleanupLauncher?.path ?? '') ||
    !HEX.test(value?.cleanupLauncher?.sha256 ?? '') ||
    !validIdentity(value?.cleanupLauncher?.identity) ||
    !Number.isSafeInteger(value?.cleanupLauncher?.bytes) ||
    value.cleanupLauncher.bytes <= 0 ||
    !isAbsolute(value?.descriptorPath ?? '') ||
    value.nativeBase !== resolve(value.nativeBase) ||
    value.nativeRoot !== resolve(value.nativeRoot) ||
    value.cleanupLauncher.path !== resolve(value.cleanupLauncher.path) ||
    value.descriptorPath !== resolve(value.descriptorPath) ||
    value.cleanupLauncher.path !==
      resolve(dirname(value.descriptorPath), 'native-publication-cleanup-helper.exe')
  ) {
    throw new Error('Native publication cleanup descriptor is invalid');
  }
  const names = value.inventory.map((entry) => entry.name).sort();
  if (JSON.stringify(names) !== JSON.stringify(Object.values(LAYOUTS[value.layout]).sort())) {
    throw new Error('Native publication cleanup descriptor inventory is invalid');
  }
  const bootstrap = value.inventory.find((entry) => entry.name === LAYOUTS[value.layout].bootstrap);
  if (
    bootstrap === undefined ||
    bootstrap.sha256 !== value.cleanupLauncher.sha256 ||
    bootstrap.bytes !== value.cleanupLauncher.bytes
  ) {
    throw new Error('Native publication cleanup launcher is not bound to the bootstrap');
  }
  for (const entry of value.inventory) {
    if (
      !validIdentity(entry.identity) ||
      !HEX.test(entry.sha256 ?? '') ||
      !Number.isSafeInteger(entry.bytes) ||
      entry.bytes <= 0 ||
      entry.path !== resolve(value.nativeRoot, entry.name)
    ) {
      throw new Error('Native publication cleanup descriptor entry is invalid');
    }
  }
  return value;
}

async function removeFailedPublication(
  nativeBase,
  nativeRoot,
  cleanupLauncherPath,
  layout,
  initializedRoot,
  protectedRoot,
) {
  if (!existsSync(nativeRoot)) return;
  if (protectedRoot) verifyNativeAcl(nativeRoot);
  else if (initializedRoot) verifyInitializedNativeAcl(nativeRoot);
  const root = await lstat(nativeRoot, { bigint: true });
  if (!root.isDirectory() || root.isSymbolicLink())
    throw new Error('Failed native root identity is invalid');
  const inventory = await inventoryPartialRoot(nativeRoot, layout);
  const launcher = await fileIdentity(cleanupLauncherPath);
  await removeExactPublication(
    {
      nativeBase,
      nativeRoot,
      rootIdentity: statIdentity(root),
      inventory,
      cleanupLauncher: { path: cleanupLauncherPath, ...launcher },
    },
    launcher,
  );
}

async function removeExactPublication(descriptor, launcher) {
  const request = {
    path: descriptor.nativeRoot,
    rootIdentity: descriptor.rootIdentity,
    entries: descriptor.inventory.map(({ name, identity }) => ({
      relativePath: name,
      directory: false,
      identity,
    })),
  };
  const result = launchVerifiedChildSync({
    bootstrap: { path: descriptor.cleanupLauncher.path, ...launcher },
    child: {
      path: descriptor.cleanupLauncher.path,
      ...launcher,
      arguments: ['--remove-exact-owned-tree-v1'],
    },
    timeoutMs: 30_000,
    input: Buffer.from(`${JSON.stringify(request)}\n`),
    maxBuffer: 4096,
  });
  if (result.error !== undefined || result.signal !== null || result.status !== 0) {
    throw subprocessFailure('Native publication handle-bound cleanup', result);
  }
  if (result.stderr !== '' || result.stdout.trim() !== '{"result":"deleted"}') {
    throw new Error('Native publication cleanup result is invalid');
  }
}

async function verifyNativeRoot(nativeRoot, expectedUserSid, layout = 'producer') {
  const userSid = verifyNativeAcl(nativeRoot, expectedUserSid);
  const root = await lstat(nativeRoot, { bigint: true });
  if (!root.isDirectory() || root.isSymbolicLink() || root.nlink !== 1n) {
    throw new Error('Native publication root identity is invalid');
  }
  return {
    userSid,
    rootIdentity: statIdentity(root),
    inventory: await inventoryRoot(nativeRoot, layout),
  };
}

async function inventoryRoot(nativeRoot, layout = 'producer') {
  const files = LAYOUTS[layout];
  if (files === undefined) throw new Error('Native publication layout is invalid');
  const names = (await readdir(nativeRoot)).sort();
  if (JSON.stringify(names) !== JSON.stringify(Object.values(files).sort())) {
    throw new Error('Native publication inventory is not exact');
  }
  return identities(nativeRoot, names);
}

async function inventoryPartialRoot(nativeRoot, layout) {
  const expected = new Set(Object.values(LAYOUTS[layout] ?? {}));
  const names = (await readdir(nativeRoot)).sort();
  if (names.some((name) => !expected.has(name))) {
    throw new Error('Failed native publication inventory is not owned');
  }
  return identities(nativeRoot, names);
}

function identities(nativeRoot, names) {
  return Promise.all(
    names.map(async (name) => ({
      name,
      path: resolve(nativeRoot, name),
      ...(await fileIdentity(resolve(nativeRoot, name))),
    })),
  );
}

async function fileIdentity(path) {
  const before = await lstat(path, { bigint: true });
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1n) {
    throw new Error(`Native publication file identity is invalid: ${basename(path)}`);
  }
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat({ bigint: true });
    const bytes = await handle.readFile();
    const after = await lstat(path, { bigint: true });
    if (
      statIdentity(before) !== statIdentity(opened) ||
      statIdentity(after) !== statIdentity(opened) ||
      opened.size !== BigInt(bytes.length) ||
      opened.mtimeNs !== after.mtimeNs ||
      opened.ctimeNs !== after.ctimeNs
    ) {
      throw new Error(`Native publication file changed: ${basename(path)}`);
    }
    return {
      bytes: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
      identity: statIdentity(opened),
    };
  } finally {
    await handle.close();
  }
}

async function removeCleanupFile(path) {
  const metadata = await lstat(path, { bigint: true });
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1n) {
    throw new Error('Native publication cleanup support file identity is invalid');
  }
  await rm(path, { force: false });
}

async function removeEmptyBase(path) {
  try {
    await rmdir(path);
  } catch (error) {
    if (!['ENOENT', 'ENOTEMPTY', 'EEXIST'].includes(error?.code)) throw error;
  }
}

function statIdentity(stat) {
  return `${stat.dev.toString()}:${stat.ino.toString()}`;
}
function validIdentity(value) {
  return /^[0-9]+:[0-9]+$/u.test(value ?? '');
}

function initializeNativeRoot(path) {
  runAclScript(path, 'initialize');
}
function protectNativeRoot(path) {
  runAclScript(path, 'protect');
}
function verifyInitializedNativeAcl(path) {
  return runAclScript(path, 'verify-initial').trim();
}
function verifyNativeAcl(path, expectedUserSid) {
  return runAclScript(path, 'verify', expectedUserSid).trim();
}

function runAclScript(path, mode, expectedUserSid = '') {
  const script = String.raw`
$ErrorActionPreference='Stop'
$root=$env:TQ_NATIVE_ROOT
$userSid=([Security.Principal.WindowsIdentity]::GetCurrent()).User
if($env:TQ_EXPECTED_USER_SID -and $userSid.Value -cne $env:TQ_EXPECTED_USER_SID){exit 20}
$admin=[Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
$system=[Security.Principal.SecurityIdentifier]::new('S-1-5-18')
function Set-ExactAcl($item,$inheritance){
  $acl=if($item.PSIsContainer){New-Object Security.AccessControl.DirectorySecurity}else{New-Object Security.AccessControl.FileSecurity}
  $acl.SetOwner($admin);$acl.SetAccessRuleProtection($true,$false)
  foreach($entry in @(@($system,2032127),@($admin,2032127),@($userSid,1179817))){
    $rule=New-Object Security.AccessControl.FileSystemAccessRule($entry[0],[Security.AccessControl.FileSystemRights]$entry[1],$inheritance,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow)
    [void]$acl.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $item.FullName -AclObject $acl
}
function Test-ExactAcl($item,$inheritance){
  if(($item.Attributes-band [IO.FileAttributes]::ReparsePoint)-ne 0){exit 21}
  $acl=Get-Acl -LiteralPath $item.FullName
  if(-not $acl.AreAccessRulesProtected){exit 22}
  $owner=([Security.Principal.NTAccount]$acl.Owner).Translate([Security.Principal.SecurityIdentifier]).Value
  if($owner-cne $admin.Value){exit 23}
  $rules=@($acl.Access)
  if($rules.Count-ne 3){exit 24}
  $expected=@{}
  $expected[$system.Value]=2032127;$expected[$admin.Value]=2032127;$expected[$userSid.Value]=1179817
  $seen=@{}
  foreach($rule in $rules){
    $sid=$rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
    $mask=([uint32][int32]$rule.FileSystemRights)
    if($rule.IsInherited -or $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or -not $expected.ContainsKey($sid) -or $mask -ne [uint32]$expected[$sid] -or $rule.InheritanceFlags -ne $inheritance -or $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None -or $seen.ContainsKey($sid)){exit 25}
    $seen[$sid]=$true
  }
  if($seen.Count-ne 3){exit 26}
}
$none=[Security.AccessControl.InheritanceFlags]::None
$inherited=[Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
if($env:TQ_ACL_MODE-ceq 'initialize'){
  $item=Get-Item -LiteralPath $root -Force;Set-ExactAcl $item $inherited;Test-ExactAcl $item $inherited
}elseif($env:TQ_ACL_MODE-ceq 'verify-initial'){
  $item=Get-Item -LiteralPath $root -Force;Test-ExactAcl $item $inherited
}else{
  $items=@(Get-ChildItem -LiteralPath $root -Force)+(Get-Item -LiteralPath $root -Force)
  if($env:TQ_ACL_MODE-ceq 'protect'){foreach($item in $items){Set-ExactAcl $item $none}}
  foreach($item in $items){Test-ExactAcl $item $none}
}
[Console]::Out.Write($userSid.Value)
`;
  const result = spawnSync(
    resolve(
      process.env.SystemRoot ?? 'C:/Windows',
      'System32/WindowsPowerShell/v1.0/powershell.exe',
    ),
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script],
    {
      env: sanitizedSubprocessEnvironment(process.env, {
        TQ_NATIVE_ROOT: path,
        TQ_ACL_MODE: mode,
        TQ_EXPECTED_USER_SID: expectedUserSid,
      }),
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30_000,
      maxBuffer: 16 * 1024,
    },
  );
  if (result.error !== undefined || result.signal !== null || result.status !== 0) {
    throw subprocessFailure(`Native publication ACL ${mode}`, result);
  }
  if (result.stderr !== '' || !/^S-[0-9-]+$/u.test(result.stdout)) {
    throw new Error(`Native publication ACL ${mode} result is invalid`);
  }
  return result.stdout;
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const mode = process.argv[2];
  if (mode === 'publish-bundle') {
    const descriptor = await publishAcceptanceNative({
      buildId: valueAfter('--build-id'),
      sourceRoot: resolve(valueAfter('--source') ?? ''),
      outputRoot: resolve(valueAfter('--output') ?? ''),
      layout: 'bundle',
    });
    console.log(JSON.stringify(descriptor));
  } else if (mode === 'cleanup') {
    console.log(
      JSON.stringify(await cleanupAcceptanceNativeDescriptor(valueAfter('--descriptor') ?? '')),
    );
  } else {
    throw new Error('Expected publish-bundle or cleanup');
  }
}
