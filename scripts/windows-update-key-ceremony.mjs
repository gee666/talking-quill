import { spawnSync } from 'node:child_process';
import { createHash, createPublicKey, generateKeyPairSync, verify } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createWindowsUpdateNativeSigner } from './windows-update-native-signer.mjs';

const root = resolve(import.meta.dirname, '..');
const version = '0.0.69';
const architecture = 'x64';
const secretDirectory = resolve(root, 'tmp/release-secrets/0.0.69-update-key');
const privateKeyPath = resolve(secretDirectory, 'windows-update-private-key.pkcs8.der');
const publicKeyPath = resolve(root, 'build/windows-update-public-key.sec1');
const evidencePath = resolve(root, 'docs/evidence/windows-update-key-ceremony-0.0.69-x64.json');
const nativeRoot = resolve(root, 'helper/target/x86_64-pc-windows-msvc/release');
const ceremonyNativeSha256 = Object.freeze({
  signerSha256: '43c9d4d00cbf02a7e769ec598e23dd1993b8cba2c71874b9268e6f1e02c8ca63',
  brokerSha256: 'd3eb07dfb70f8a9590195cf02e9f5a415c48865fd3cf7c68afe14e979822bb56',
  bootstrapSha256: 'f72100056a34b78b708d3882d869acf78414a106ffa25e7f74b6fac48e5b1506',
});

export function performWindowsUpdateKeyCeremony(options = {}) {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('The Windows updater key ceremony requires native Windows x64');
  }
  if (existsSync(evidencePath)) {
    throw new Error('The 0.0.69 Windows updater key ceremony is already complete');
  }
  if (options.resume === true) {
    protectDirectory(secretDirectory);
  } else {
    mkdirSync(resolve(root, 'tmp/release-secrets'), { recursive: true });
    mkdirSync(secretDirectory, { recursive: false });
    protectDirectory(secretDirectory);

    const { privateKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const privateDer = privateKey.export({ format: 'der', type: 'pkcs8' });
    try {
      writeFileSync(privateKeyPath, privateDer, { flag: 'wx', mode: 0o400 });
    } finally {
      privateDer.fill(0);
    }
  }
  const acl = protectPrivateKey(privateKeyPath);

  const nativePaths = prepareNativeCopies(options);
  const native = createWindowsUpdateNativeSigner({
    privateKeyPath,
    ...nativePaths,
    signerSha256: options.signerSha256 ?? ceremonyNativeSha256.signerSha256,
    brokerSha256: options.brokerSha256 ?? ceremonyNativeSha256.brokerSha256,
    bootstrapSha256: options.bootstrapSha256 ?? ceremonyNativeSha256.bootstrapSha256,
  });
  const probe = Buffer.from('talking-quill/windows-update-key-ceremony/0.0.69/x64/v1\0', 'utf8');
  const signed = native.sign(probe);
  if (signed.publicKeySec1.length !== 65 || signed.publicKeySec1[0] !== 4) {
    throw new Error('Native signer did not derive an uncompressed P-256 public key');
  }
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    signed.publicKeySec1,
  ]);
  if (
    !verify(
      'sha256',
      probe,
      createPublicKey({ key: spki, format: 'der', type: 'spki' }),
      signed.signatureDer,
    )
  ) {
    throw new Error('Native signer ceremony proof did not verify');
  }

  const publicKeySha256 = createHash('sha256').update(signed.publicKeySec1).digest('hex');
  writeFileSync(publicKeyPath, `${signed.publicKeySec1.toString('hex')}\n`, 'utf8');
  mkdirSync(resolve(evidencePath, '..'), { recursive: true });
  const evidence = {
    schemaVersion: 1,
    purpose: 'talking-quill/windows-update-trust-root-key-ceremony',
    releaseVersion: version,
    architecture,
    performedAtUtc: new Date().toISOString(),
    generator: `Node.js ${process.version} crypto.generateKeyPairSync prime256v1`,
    privateKeyEncoding: 'PKCS8 DER',
    publicKeyEncoding: 'SEC1 uncompressed',
    publicKeySha256,
    acl: {
      inheritance: acl.inheritance,
      rights: acl.rights,
      principals: acl.principals,
    },
    derivation: {
      implementation: 'talking-quill native acceptance signer and verified-child broker',
      probeSignatureVerified: true,
      signer: publicIdentity(native.identities.signer),
      broker: publicIdentity(native.identities.broker),
      bootstrap: publicIdentity(native.identities.bootstrap),
    },
    privateMaterialRecorded: false,
  };
  writeFileSync(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`, { flag: 'wx' });
  return Object.freeze({ privateKeyPath, publicKeySha256, acl, evidencePath });
}

function publicIdentity(identity) {
  return { sha256: identity.sha256, bytes: identity.bytes };
}

function prepareNativeCopies(options) {
  const destination = resolve(secretDirectory, 'native');
  const paths = {
    signerPath: resolve(destination, 'talking-quill-acceptance-signer.exe'),
    brokerPath: resolve(destination, 'talking-quill-windows-acceptance-broker.exe'),
    bootstrapPath: resolve(destination, 'talking-quill-helper.exe'),
  };
  if (!existsSync(destination)) {
    mkdirSync(destination, { recursive: false });
    copyFileSync(
      options.signerPath ?? resolve(nativeRoot, 'talking-quill-acceptance-signer.exe'),
      paths.signerPath,
    );
    copyFileSync(
      options.brokerPath ?? resolve(nativeRoot, 'talking-quill-windows-acceptance-broker.exe'),
      paths.brokerPath,
    );
    copyFileSync(
      options.bootstrapPath ??
        resolve(root, 'helper/target/x86_64-pc-windows-msvc/debug/talking-quill-helper.exe'),
      paths.bootstrapPath,
    );
  }
  applyAcl(destination, '(OI)(CI)RX');
  for (const path of Object.values(paths)) applyAcl(path, 'RX');
  return paths;
}

function protectDirectory(path) {
  applyAcl(path, '(OI)(CI)F');
}

function protectPrivateKey(path) {
  applyAcl(path, 'R');
  const stdout = runAclScript(
    String.raw`
$path=$TargetPath
$current=[Security.Principal.WindowsIdentity]::GetCurrent().User
$actual=Get-Acl -LiteralPath $path
$rules=@($actual.Access)
$expected=@($current.Value,'S-1-5-18','S-1-5-32-544')|Sort-Object
$actualSids=@($rules|ForEach-Object {$_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value})|Sort-Object
$read=[int]([Security.AccessControl.FileSystemRights]::Read -bor [Security.AccessControl.FileSystemRights]::Synchronize)
if(-not $actual.AreAccessRulesProtected -or $rules.Count -ne 3 -or (Compare-Object $expected $actualSids) -or ($rules|Where-Object {$_.IsInherited -or $_.AccessControlType -ne 'Allow' -or [int]$_.FileSystemRights -ne $read})){throw 'private key ACL verification failed'}
[ordered]@{inheritance='disabled';rights='read';principals=@('SYSTEM','Administrators','current-user');currentUserSid=$current.Value}|ConvertTo-Json -Compress
`,
    path,
  );
  return JSON.parse(stdout);
}

function applyAcl(path, rights) {
  const sid = currentUserSid();
  const icacls = resolve(process.env.SystemRoot ?? 'C:/Windows', 'System32/icacls.exe');
  const result = spawnSync(
    icacls,
    [
      path,
      '/inheritance:r',
      '/grant:r',
      `*S-1-5-18:${rights}`,
      `*S-1-5-32-544:${rights}`,
      `*${sid}:${rights}`,
    ],
    { encoding: 'utf8', windowsHide: true, maxBuffer: 8 * 1024 },
  );
  if (result.status !== 0 || result.signal !== null || result.error !== undefined) {
    throw new Error('Windows updater key ACL operation failed');
  }
}

function currentUserSid() {
  return runAclScript('[Security.Principal.WindowsIdentity]::GetCurrent().User.Value', '');
}

function runAclScript(script, path) {
  const powershell = resolve(
    process.env.SystemRoot ?? 'C:/Windows',
    'System32/WindowsPowerShell/v1.0/powershell.exe',
  );
  const command = `& { param([string]$TargetPath)\n${script}\n}`;
  const result = spawnSync(
    powershell,
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command, path],
    {
      encoding: 'utf8',
      windowsHide: true,
      maxBuffer: 8 * 1024,
    },
  );
  if (result.status !== 0 || result.signal !== null || result.error !== undefined) {
    throw new Error('Windows updater key ACL operation failed');
  }
  return result.stdout.trim();
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const result = performWindowsUpdateKeyCeremony({
    signerPath: valueAfter('--native-signer'),
    brokerPath: valueAfter('--native-broker'),
    bootstrapPath: valueAfter('--native-bootstrap'),
    signerSha256: valueAfter('--native-signer-sha256'),
    brokerSha256: valueAfter('--native-broker-sha256'),
    bootstrapSha256: valueAfter('--native-bootstrap-sha256'),
    resume: process.argv.includes('--resume'),
  });
  console.log(
    JSON.stringify({
      result: 'passed',
      privateKeyPath: result.privateKeyPath,
      publicKeySha256: result.publicKeySha256,
      acl: result.acl,
      evidencePath: result.evidencePath,
    }),
  );
}
