import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
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
  const publicationId = randomBytes(32).toString('hex');
  const nativeRoot = resolve(nativeBase, publicationId);
  const cleanupLauncherPath = resolve(outputRoot, 'native-publication-cleanup-helper.exe');
  let nativeBaseCreated = false;
  let nativeBaseIdentity;
  let nativeRootIdentity;
  let cleanupLauncher;
  let cleanupLauncherCreated = false;
  let initializedRoot = false;
  let protectedRoot = false;
  try {
    await requireDirectory(resolve(programData));
    try {
      nativeBaseIdentity = await directoryIdentity(nativeBase);
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
      await mkdir(nativeBase, { recursive: false, mode: 0o700 });
      nativeBaseCreated = true;
      nativeBaseIdentity = await directoryIdentity(nativeBase);
    }
    await copyFile(
      resolve(sourceRoot, files.bootstrap),
      cleanupLauncherPath,
      constants.COPYFILE_EXCL,
    );
    cleanupLauncherCreated = true;
    cleanupLauncher = await fileIdentity(cleanupLauncherPath);
    await mkdir(nativeRoot, { recursive: false, mode: 0o700 });
    nativeRootIdentity = await directoryIdentity(nativeRoot);
    initializeNativeRoot(nativeRoot);
    initializedRoot = true;
    for (const name of Object.values(files)) {
      await copyFile(resolve(sourceRoot, name), resolve(nativeRoot, name), constants.COPYFILE_EXCL);
    }
    protectNativeRoot(nativeRoot);
    protectedRoot = true;
    const { userSid, rootIdentity, inventory } = await verifyNativeRoot(nativeRoot);
    const descriptorPath = resolve(outputRoot, 'native-publication-cleanup.json');
    const descriptor = {
      schemaVersion: 1,
      purpose: PURPOSE,
      buildId,
      publicationId,
      layout,
      removeNativeBase: nativeBaseCreated,
      nativeBase,
      nativeBaseIdentity,
      nativeRoot,
      userSid,
      rootIdentity,
      inventory,
      cleanupLauncher: { path: cleanupLauncherPath, ...cleanupLauncher },
      descriptorPath,
    };
    const descriptorBytes = Buffer.from(`${JSON.stringify(descriptor)}\n`);
    await writeFile(descriptorPath, descriptorBytes, { flag: 'wx', mode: 0o600 });
    return Object.freeze({
      ...descriptor,
      descriptorSha256: createHash('sha256').update(descriptorBytes).digest('hex'),
    });
  } catch (error) {
    const cleanupErrors = [];
    if (nativeRootIdentity !== undefined && cleanupLauncher !== undefined) {
      try {
        await removeFailedPublication(
          nativeRoot,
          nativeRootIdentity,
          cleanupLauncherPath,
          cleanupLauncher,
          layout,
          initializedRoot,
          protectedRoot,
        );
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    if (cleanupLauncherCreated && !existsSync(nativeRoot)) {
      try {
        await removeCleanupFile(cleanupLauncherPath, cleanupLauncher);
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    if (nativeBaseCreated && nativeBaseIdentity !== undefined && !existsSync(nativeRoot)) {
      try {
        await removeEmptyBase(nativeBase, nativeBaseIdentity);
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    if (cleanupErrors.length > 0) {
      throw new AggregateError([error, ...cleanupErrors], 'Native publication and cleanup failed');
    }
    throw error;
  }
}

export async function cleanupAcceptanceNativeDescriptor(descriptorPath, options) {
  if (!isAbsolute(descriptorPath)) {
    throw new Error('Native publication descriptor path must be absolute');
  }
  const absolute = resolve(descriptorPath);
  const descriptor = JSON.parse(await readFile(absolute, 'utf8'));
  if (resolve(descriptor?.descriptorPath ?? '') !== absolute) {
    throw new Error('Native publication descriptor path is not canonical');
  }
  return cleanupAcceptanceNative(
    { ...descriptor, descriptorSha256: options?.descriptorSha256 },
    options,
  );
}

export async function cleanupAcceptanceNative(
  descriptor,
  { programData = process.env.ProgramData ?? 'C:/ProgramData' } = {},
) {
  const checked = validateDescriptor(descriptor);
  if (resolve(checked.nativeBase) !== resolve(programData, 'Talking Quill Acceptance Native')) {
    throw new Error('Native publication cleanup base is unauthorized');
  }
  await requireDirectory(resolve(programData));
  let nativeBaseIdentity;
  try {
    nativeBaseIdentity = await directoryIdentity(checked.nativeBase);
  } catch (error) {
    if (error?.code !== 'ENOENT' || !checked.removeNativeBase || existsSync(checked.nativeRoot)) {
      throw error;
    }
  }
  if (nativeBaseIdentity !== undefined && nativeBaseIdentity !== checked.nativeBaseIdentity) {
    throw new Error('Native publication cleanup base changed');
  }
  const descriptorFile = await fileIdentity(checked.descriptorPath);
  if (descriptorFile.sha256 !== checked.descriptorSha256) {
    throw new Error('Native publication cleanup descriptor changed');
  }
  let launcher;
  if (existsSync(checked.cleanupLauncher.path)) {
    launcher = await fileIdentity(checked.cleanupLauncher.path);
    requireSameFileIdentity(
      launcher,
      checked.cleanupLauncher,
      'Native publication cleanup launcher changed',
    );
  }
  if (existsSync(checked.nativeRoot)) {
    if (launcher === undefined) {
      throw new Error('Native publication cleanup launcher is absent');
    }
    const verified = await verifyNativeRoot(checked.nativeRoot, checked.userSid, checked.layout);
    if (
      verified.rootIdentity !== checked.rootIdentity ||
      JSON.stringify(verified.inventory) !== JSON.stringify(checked.inventory)
    ) {
      throw new Error('Native publication identity or inventory changed before cleanup');
    }
    await removeExactPublication(checked, launcher);
    if (existsSync(checked.nativeRoot)) throw new Error('Native publication cleanup left residue');
  }
  const cleanupErrors = [];
  try {
    await removeCleanupFile(checked.cleanupLauncher.path, launcher);
  } catch (error) {
    cleanupErrors.push(error);
  }
  if (checked.removeNativeBase && nativeBaseIdentity !== undefined) {
    try {
      await removeEmptyBase(checked.nativeBase, nativeBaseIdentity, true);
    } catch (error) {
      cleanupErrors.push(error);
    }
  }
  if (cleanupErrors.length > 0) {
    throw new AggregateError(cleanupErrors, 'Native publication support cleanup failed');
  }
  await removeCleanupFile(checked.descriptorPath, descriptorFile);
  return Object.freeze({ result: 'deleted' });
}

export function validateDescriptor(value) {
  if (
    value?.schemaVersion !== 1 ||
    value?.purpose !== PURPOSE ||
    !HEX.test(value?.buildId ?? '') ||
    !HEX.test(value?.publicationId ?? '') ||
    LAYOUTS[value?.layout] === undefined ||
    typeof value?.removeNativeBase !== 'boolean' ||
    !isAbsolute(value?.nativeBase ?? '') ||
    !validIdentity(value?.nativeBaseIdentity) ||
    !isAbsolute(value?.nativeRoot ?? '') ||
    resolve(value.nativeRoot) !== resolve(value.nativeBase, value.publicationId) ||
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
    !HEX.test(value?.descriptorSha256 ?? '') ||
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
  nativeRoot,
  expectedRootIdentity,
  cleanupLauncherPath,
  expectedLauncher,
  layout,
  initializedRoot,
  protectedRoot,
) {
  if (!existsSync(nativeRoot)) return;
  const rootIdentity = await directoryIdentity(nativeRoot);
  if (rootIdentity !== expectedRootIdentity) {
    throw new Error('Failed native root identity changed');
  }
  if (protectedRoot) verifyNativeAcl(nativeRoot);
  else if (initializedRoot) verifyInitializedNativeAcl(nativeRoot);
  const inventory = await inventoryPartialRoot(nativeRoot, layout);
  if (!protectedRoot && !initializedRoot && inventory.length > 0) {
    verifyInitializedNativeAcl(nativeRoot);
  }
  const launcher = await fileIdentity(cleanupLauncherPath);
  requireSameFileIdentity(
    launcher,
    expectedLauncher,
    'Native publication cleanup launcher changed',
  );
  await removeExactPublication(
    {
      nativeRoot,
      rootIdentity,
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

async function removeCleanupFile(path, expected) {
  if (expected === undefined && !existsSync(path)) return;
  const observed = await fileIdentity(path);
  if (expected !== undefined) {
    requireSameFileIdentity(
      observed,
      expected,
      'Native publication cleanup support file identity changed',
    );
  }
  await rm(path, { force: false });
}

async function removeEmptyBase(path, expectedIdentity, requireRemoval = false) {
  try {
    if (expectedIdentity !== undefined && (await directoryIdentity(path)) !== expectedIdentity) {
      throw new Error('Native publication base identity changed');
    }
    await rmdir(path);
  } catch (error) {
    if (error?.code === 'ENOENT') return;
    if (!requireRemoval && ['ENOTEMPTY', 'EEXIST'].includes(error?.code)) return;
    if (requireRemoval && ['ENOTEMPTY', 'EEXIST'].includes(error?.code)) {
      throw new Error('Native publication base is not empty');
    }
    throw error;
  }
}

async function requireDirectory(path) {
  await directoryIdentity(path);
}

async function directoryIdentity(path) {
  const metadata = await lstat(path, { bigint: true });
  if (!metadata.isDirectory() || metadata.isSymbolicLink() || metadata.nlink !== 1n) {
    throw new Error('Native publication directory identity is invalid');
  }
  return statIdentity(metadata);
}

function requireSameFileIdentity(observed, expected, message) {
  if (
    observed.sha256 !== expected.sha256 ||
    observed.bytes !== expected.bytes ||
    observed.identity !== expected.identity
  ) {
    throw new Error(message);
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

export function runAclScript(path, mode, expectedUserSid = '') {
  const script = String.raw`
$ErrorActionPreference='Stop'
$root=$env:TQ_NATIVE_ROOT
$userSid=([Security.Principal.WindowsIdentity]::GetCurrent()).User
if($env:TQ_EXPECTED_USER_SID -and $userSid.Value -cne $env:TQ_EXPECTED_USER_SID){exit 20}
$admin=[Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
$system=[Security.Principal.SecurityIdentifier]::new('S-1-5-18')
function New-ExactAcl($item,$inheritance){
  $acl=if($item.PSIsContainer){New-Object Security.AccessControl.DirectorySecurity}else{New-Object Security.AccessControl.FileSecurity}
  $acl.SetOwner($admin);$acl.SetAccessRuleProtection($true,$false)
  foreach($entry in @(@($system,2032127),@($admin,2032127),@($userSid,1179817))){
    $rule=New-Object Security.AccessControl.FileSystemAccessRule($entry[0],[Security.AccessControl.FileSystemRights]$entry[1],$inheritance,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow)
    [void]$acl.AddAccessRule($rule)
  }
  return $acl
}
function Set-ExactAcl($item,$inheritance){
  if(($item.Attributes-band [IO.FileAttributes]::ReparsePoint)-ne 0){exit 21}
  $item.SetAccessControl((New-ExactAcl $item $inheritance))
}
function Test-ExactAcl($item,$inheritance){
  if(($item.Attributes-band [IO.FileAttributes]::ReparsePoint)-ne 0){exit 21}
  $sections=[Security.AccessControl.AccessControlSections]::Owner -bor [Security.AccessControl.AccessControlSections]::Access
  # Read the native descriptor without resolving SIDs to account names.
  $actual=$item.GetAccessControl($sections)
  if(-not $actual.AreAccessRulesProtected){exit 22}
  $expected=New-ExactAcl $item $inheritance
  $expectedSddl=$expected.GetSecurityDescriptorSddlForm($sections).Replace('D:P','D:PAI')
  if($actual.GetSecurityDescriptorSddlForm($sections)-cne $expectedSddl){exit 23}
}
$none=[Security.AccessControl.InheritanceFlags]::None
$inherited=[Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
if($env:TQ_ACL_MODE-ceq 'initialize'){
  $item=Get-Item -LiteralPath $root -Force;Set-ExactAcl $item $inherited;Test-ExactAcl $item $inherited
}elseif($env:TQ_ACL_MODE-ceq 'verify-initial'){
  $item=Get-Item -LiteralPath $root -Force;Test-ExactAcl $item $inherited
}else{
  $items=@(Get-ChildItem -LiteralPath $root -Force)+@(Get-Item -LiteralPath $root -Force)
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
      // Cold hosted PowerShell/ACL operations have exceeded the former 30s bound.
      timeout: 120_000,
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
      JSON.stringify(
        await cleanupAcceptanceNativeDescriptor(valueAfter('--descriptor') ?? '', {
          descriptorSha256: valueAfter('--descriptor-sha256'),
        }),
      ),
    );
  } else {
    throw new Error('Expected publish-bundle or cleanup');
  }
}
