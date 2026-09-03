import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  constants,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
  closeSync,
  fstatSync,
} from 'node:fs';
import { basename, resolve } from 'node:path';
import { parseNativeArchitectures } from './native-architecture.mjs';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { currentSourceIdentity } from './source-identity.mjs';

const root = resolve(import.meta.dirname, '..');
const manifestPath = resolve(root, 'helper/Cargo.toml');
const lockPath = resolve(root, 'helper/Cargo.lock');
const targetRoot = resolve(root, 'helper/target/x86_64-pc-windows-msvc/release');
const roles = Object.freeze({
  signer: 'talking-quill-acceptance-signer.exe',
  broker: 'talking-quill-windows-acceptance-broker.exe',
  bootstrap: 'talking-quill-helper.exe',
  keyTool: 'talking-quill-update-key-tool.exe',
});

let preparedChain;

export function prepareReviewedWindowsUpdateNativeChain() {
  if (preparedChain !== undefined) return preparedChain;
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Reviewed Windows update signing requires native Windows x64');
  }
  const source = currentSourceIdentity({ repositoryRoot: root, requireClean: true });
  const lock = verifyCargoLock(source.sourceCommit);
  const environment = sanitizedSubprocessEnvironment(process.env, {
    TALKING_QUILL_SOURCE_COMMIT: source.sourceCommit,
    TALKING_QUILL_SOURCE_TREE: source.sourceTree,
  });
  runCargo(
    [
      'build',
      '--manifest-path',
      manifestPath,
      '--target-dir',
      resolve(root, 'helper/target'),
      '--locked',
      '--release',
      '--target',
      'x86_64-pc-windows-msvc',
      '-p',
      'talking-quill-acceptance-signer',
      '--bin',
      'talking-quill-acceptance-signer',
      '--bin',
      'talking-quill-windows-acceptance-broker',
      '--bin',
      'talking-quill-update-key-tool',
    ],
    environment,
  );
  runCargo(
    [
      'build',
      '--manifest-path',
      manifestPath,
      '--target-dir',
      resolve(root, 'helper/target'),
      '--locked',
      '--release',
      '--target',
      'x86_64-pc-windows-msvc',
      '-p',
      'talking-quill-helper',
      '--features',
      'windows-installed-acceptance',
      '--bin',
      'talking-quill-helper',
    ],
    environment,
  );
  const after = currentSourceIdentity({ repositoryRoot: root, requireClean: true });
  if (after.sourceCommit !== source.sourceCommit || after.sourceTree !== source.sourceTree) {
    throw new Error('Source identity changed during native signing-chain preparation');
  }
  const afterLock = verifyCargoLock(source.sourceCommit);
  if (afterLock.sha256 !== lock.sha256 || afterLock.blob !== lock.blob) {
    throw new Error('Cargo.lock changed during native signing-chain preparation');
  }
  const built = Object.fromEntries(
    Object.entries(roles).map(([role, name]) => [
      role,
      verifyExecutable(resolve(targetRoot, name), name, source),
    ]),
  );
  preparedChain = publishSnapshot(source, lock, built);
  return preparedChain;
}

function runCargo(arguments_, environment) {
  const result = spawnSync('cargo.exe', arguments_, {
    cwd: root,
    env: environment,
    encoding: 'utf8',
    windowsHide: true,
    timeout: 10 * 60_000,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.error !== undefined || result.signal !== null || result.status !== 0) {
    throw new Error('Reviewed Windows update native build failed');
  }
}

function verifyCargoLock(commit) {
  const tracked = execFileSync('git', ['show', `${commit}:helper/Cargo.lock`], {
    cwd: root,
    encoding: null,
    timeout: 30_000,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const local = readFileSync(lockPath);
  if (!local.equals(tracked)) throw new Error('helper/Cargo.lock differs from the reviewed commit');
  return Object.freeze({
    sha256: createHash('sha256').update(local).digest('hex'),
    blob: execFileSync('git', ['rev-parse', `${commit}:helper/Cargo.lock`], {
      cwd: root,
      encoding: 'utf8',
      timeout: 30_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim(),
  });
}

function verifyExecutable(path, expectedName, source) {
  if (basename(path) !== expectedName) throw new Error('Native signing-chain filename is invalid');
  const before = lstatSync(path, { bigint: true });
  if (!before.isFile() || before.isSymbolicLink()) {
    throw new Error('Native signing-chain file identity is invalid');
  }
  const descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = fstatSync(descriptor, { bigint: true });
    const bytes = readFileSync(descriptor);
    const after = lstatSync(path, { bigint: true });
    if (
      opened.dev !== before.dev ||
      opened.ino !== before.ino ||
      after.dev !== opened.dev ||
      after.ino !== opened.ino ||
      opened.size !== BigInt(bytes.length) ||
      opened.mtimeNs !== after.mtimeNs ||
      opened.ctimeNs !== after.ctimeNs
    ) {
      throw new Error('Native signing-chain file changed during verification');
    }
    const architecture = parseNativeArchitectures(bytes.subarray(0, 64 * 1024), path);
    if (
      architecture?.format !== 'pe' ||
      architecture.architectures.length !== 1 ||
      architecture.architectures[0] !== 'x64'
    ) {
      throw new Error('Native signing-chain executable is not exact x64 PE');
    }
    for (const marker of [
      `TALKING_QUILL_SOURCE_COMMIT=${source.sourceCommit}`,
      `TALKING_QUILL_SOURCE_TREE=${source.sourceTree}`,
    ]) {
      if (occurrences(bytes, Buffer.from(marker, 'ascii')) !== 1) {
        throw new Error('Native signing-chain source marker is invalid');
      }
    }
    return Object.freeze({
      sourcePath: path,
      bytes: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
      volume: opened.dev.toString(),
      fileId: opened.ino.toString(),
    });
  } finally {
    closeSync(descriptor);
  }
}

function publishSnapshot(source, lock, built) {
  const base = resolve(process.env.ProgramData ?? 'C:/ProgramData', 'Talking Quill Update Signing');
  const identity = `${source.sourceTree}-${lock.sha256}`;
  const destination = resolve(base, identity);
  const provenancePath = resolve(destination, 'provenance.json');
  const provenance = {
    schemaVersion: 1,
    purpose: 'talking-quill/reviewed-windows-update-native-chain',
    sourceCommit: source.sourceCommit,
    sourceTree: source.sourceTree,
    cargoLockBlob: lock.blob,
    cargoLockSha256: lock.sha256,
    target: 'x86_64-pc-windows-msvc',
    profile: 'release',
    executables: Object.fromEntries(
      Object.entries(built).map(([role, value]) => [
        role,
        { name: roles[role], bytes: value.bytes, sha256: value.sha256 },
      ]),
    ),
  };
  if (!existsSync(destination)) {
    mkdirSync(base, { recursive: true });
    const pending = `${destination}.pending-${String(process.pid)}`;
    rmSync(pending, { recursive: true, force: true });
    mkdirSync(pending, { recursive: false });
    for (const [role, value] of Object.entries(built)) {
      copyFileSync(value.sourcePath, resolve(pending, roles[role]), constants.COPYFILE_EXCL);
    }
    writeFileSync(resolve(pending, 'provenance.json'), `${JSON.stringify(provenance)}\n`, {
      flag: 'wx',
    });
    protectSnapshot(pending);
    renameSync(pending, destination);
  }
  verifySnapshotProtection(destination);
  const recorded = JSON.parse(readFileSync(provenancePath, 'utf8'));
  if (JSON.stringify(recorded) !== JSON.stringify(provenance)) {
    throw new Error('Reviewed native-chain provenance does not match this source');
  }
  const identities = Object.fromEntries(
    Object.entries(roles).map(([role, name]) => {
      const verified = verifyExecutable(resolve(destination, name), name, source);
      if (verified.sha256 !== built[role].sha256 || verified.bytes !== built[role].bytes) {
        throw new Error('Published native-chain identity differs from reviewed build');
      }
      return [
        role,
        { path: resolve(destination, name), sha256: verified.sha256, bytes: verified.bytes },
      ];
    }),
  );
  return Object.freeze({ ...identities, source, provenancePath, cargoLock: lock });
}

function protectSnapshot(path) {
  const script = String.raw`
$ErrorActionPreference='Stop'
$path=$env:TQ_NATIVE_SNAPSHOT_PATH
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
& "$env:SystemRoot\System32\icacls.exe" $path '/inheritance:r' '/grant:r' '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' "*$($sid):(OI)(CI)RX" | Out-Null
if($LASTEXITCODE-ne 0){throw 'ACL publication failed'}
$children=@(Get-ChildItem -LiteralPath $path -Force)
foreach($child in $children){
  & "$env:SystemRoot\System32\icacls.exe" $child.FullName '/inheritance:r' '/grant:r' '*S-1-5-18:F' '*S-1-5-32-544:F' "*$($sid):RX" | Out-Null
  if($LASTEXITCODE-ne 0){throw 'child ACL publication failed'}
}
$items=@(Get-Item -LiteralPath $path)+$children
foreach($item in $items){$acl=Get-Acl -LiteralPath $item.FullName;if(-not $acl.AreAccessRulesProtected){throw 'ACL inheritance'};if(($item.Attributes-band [IO.FileAttributes]::ReparsePoint)-ne 0){throw 'reparse'}}
`;
  const result = spawnSync(
    resolve(
      process.env.SystemRoot ?? 'C:/Windows',
      'System32/WindowsPowerShell/v1.0/powershell.exe',
    ),
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script],
    {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30_000,
      env: { ...process.env, TQ_NATIVE_SNAPSHOT_PATH: path },
    },
  );
  if (result.error !== undefined || result.signal !== null || result.status !== 0) {
    throw new Error(
      `Reviewed native-chain protected publication failed (${String(result.status)}): ${result.stderr.trim()}`,
    );
  }
}

export function generateProtectedWindowsUpdateKey(keyPath) {
  return runKeyTool('--generate-protected-key-v1', keyPath);
}

export function validateProtectedWindowsUpdateKey(keyPath) {
  return runKeyTool('--validate-protected-key-v1', keyPath);
}

export function deleteProtectedWindowsUpdateKey(keyPath) {
  return runKeyTool('--delete-protected-key-v1', keyPath);
}

function runKeyTool(mode, keyPath, input) {
  const chain = prepareReviewedWindowsUpdateNativeChain();
  const result = spawnSync(chain.keyTool.path, [mode, keyPath], {
    cwd: root,
    env: sanitizedSubprocessEnvironment(),
    input,
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30_000,
    maxBuffer: 4096,
  });
  if (
    result.error !== undefined ||
    result.signal !== null ||
    result.status !== 0 ||
    result.stderr !== ''
  ) {
    throw new Error('Native protected update-key operation failed');
  }
  return JSON.parse(result.stdout);
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const [mode] = process.argv.slice(2);
  const keyPath = valueAfter('--key-path');
  if (!keyPath || !['key-import', 'key-delete', 'key-validate', 'key-generate'].includes(mode)) {
    throw new Error(
      'Usage: windows-update-native-chain <key-import|key-delete|key-validate|key-generate> --key-path <absolute-path>',
    );
  }
  if (mode === 'key-import') {
    const encoded = readFileSync(0, { encoding: 'utf8' }).trim();
    if (!/^[A-Za-z0-9+/]+={0,2}$/u.test(encoded) || encoded.length > 1024) {
      throw new Error('Protected update-key import input is invalid');
    }
    const secret = Buffer.from(encoded, 'base64');
    try {
      const result = runKeyTool('--import-protected-key-v1', keyPath, secret);
      const publicKeySha256 = createHash('sha256')
        .update(Buffer.from(result.publicKeySec1Hex, 'hex'))
        .digest('hex');
      console.log(
        JSON.stringify({ result: 'imported', keyPath: resolve(keyPath), publicKeySha256 }),
      );
    } finally {
      secret.fill(0);
    }
  } else {
    const result =
      mode === 'key-delete'
        ? deleteProtectedWindowsUpdateKey(keyPath)
        : mode === 'key-validate'
          ? validateProtectedWindowsUpdateKey(keyPath)
          : generateProtectedWindowsUpdateKey(keyPath);
    if (result.publicKeySec1Hex) {
      result.publicKeySha256 = createHash('sha256')
        .update(Buffer.from(result.publicKeySec1Hex, 'hex'))
        .digest('hex');
      delete result.publicKeySec1Hex;
    }
    console.log(JSON.stringify(result));
  }
}

function verifySnapshotProtection(path) {
  const script = String.raw`
$ErrorActionPreference='Stop'
$path=$env:TQ_NATIVE_SNAPSHOT_PATH
$current=[Security.Principal.WindowsIdentity]::GetCurrent().User
$expected=@{}
$expected['S-1-5-18']=2032127
$expected['S-1-5-32-544']=2032127
$expected[$current.Value]=1179817
$items=@(Get-Item -LiteralPath $path)+(Get-ChildItem -LiteralPath $path -Force)
foreach($item in $items){
  if(($item.Attributes-band [IO.FileAttributes]::ReparsePoint)-ne 0){throw 'snapshot reparse'}
  $acl=Get-Acl -LiteralPath $item.FullName
  if(-not $acl.AreAccessRulesProtected -or $acl.Owner -ne $current.Translate([Security.Principal.NTAccount]).Value){throw 'snapshot owner or inheritance'}
  $rules=@($acl.Access)
  if($rules.Count-ne 3){throw 'snapshot ace count'}
  $seen=@{}
  foreach($rule in $rules){
    $sid=$rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
    if($rule.IsInherited -or $rule.AccessControlType-ne 'Allow' -or -not $expected.ContainsKey($sid) -or $seen.ContainsKey($sid) -or [int]$rule.FileSystemRights-ne $expected[$sid]){throw 'snapshot ace policy'}
    $seen[$sid]=$true
  }
  if($seen.Count-ne $expected.Count){throw 'snapshot principal set'}
}
`;
  const result = spawnSync(
    resolve(
      process.env.SystemRoot ?? 'C:/Windows',
      'System32/WindowsPowerShell/v1.0/powershell.exe',
    ),
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script],
    {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30_000,
      env: { ...process.env, TQ_NATIVE_SNAPSHOT_PATH: path },
    },
  );
  if (result.error !== undefined || result.signal !== null || result.status !== 0) {
    throw new Error('Reviewed native-chain snapshot ACL is invalid');
  }
}

function occurrences(bytes, marker) {
  let count = 0;
  let offset = 0;
  while ((offset = bytes.indexOf(marker, offset)) >= 0) {
    count += 1;
    offset += marker.length;
  }
  return count;
}
