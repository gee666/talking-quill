#!/usr/bin/env node
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  randomBytes,
  sign,
  verify,
} from 'node:crypto';
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, relative, resolve, sep } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { hostname, userInfo } from 'node:os';
import { pathToFileURL } from 'node:url';
import {
  MACOS_R11_CHECKPOINTS,
  canonicalJson,
  sha256File,
  writeMacosR11Evidence,
} from './macos-r11-evidence.mjs';

const APP = '/Applications/Talking Quill.app';
const OWNER_RELATIVE =
  'Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner';
const GATEWAY_RELATIVE = 'Contents/Resources/helper/talking-quill-helper';
const BRIDGE_RELATIVE = 'Contents/MacOS/talking-quill-macos-service-bridge';
const POLICY_RELATIVE = 'Contents/Resources/keyboard-owner-r5m.json';
const RELEASE_METADATA_RELATIVE = 'Contents/Resources/keyboard-owner-release-v1.json';

async function main() {
  const args = process.argv.slice(2);
  if (args[0] === '--print-checkpoints') {
    process.stdout.write(`${JSON.stringify(MACOS_R11_CHECKPOINTS, null, 2)}\n`);
    return;
  }
  if (args[0] === '--checkpoint') {
    submitCheckpoint(parse(args.slice(1)));
    return;
  }
  if (process.platform !== 'darwin') {
    throw new Error('The macOS R11 lifecycle must execute on a native macOS runner');
  }
  const options = parse(args);
  const arch = required(options, 'arch');
  const expectedArch = process.arch === 'x64' ? 'x64' : process.arch === 'arm64' ? 'arm64' : null;
  if (arch !== expectedArch)
    throw new Error(`Native runner architecture ${process.arch} cannot test ${arch}`);
  const signingMode = required(options, 'signing-mode');
  if (signingMode !== 'self-signed' && signingMode !== 'adhoc') {
    throw new Error('Expected --signing-mode self-signed|adhoc');
  }
  const candidateDmg = existing(required(options, 'candidate-dmg'));
  const candidateZip = existing(required(options, 'candidate-zip'));
  const baselineZip = existing(required(options, 'baseline-zip'));
  const provenance = JSON.parse(readFileSync(existing(required(options, 'provenance')), 'utf8'));
  const evidencePath = resolve(required(options, 'evidence'));
  const checkpointRoot = resolve(required(options, 'checkpoint-root'));
  const workRoot = resolve(options.get('work-root') ?? `tmp/macos-r11-${process.pid}`);
  const manualTimeout = Number(options.get('manual-timeout-seconds') ?? '1800') * 1_000;
  assertWorkRoot(workRoot);
  mkdirSync(workRoot, { recursive: true, mode: 0o700 });
  mkdirSync(checkpointRoot, { recursive: true, mode: 0o700 });
  const before = artifactSnapshot({ candidateDmg, candidateZip, baselineZip });
  verifyProvenance(provenance, arch, before.baselineZip.sha256Before);
  const sessionBindingSha256 = createHash('sha256')
    .update(
      canonicalJson({
        repository: requiredEnvironment('GITHUB_REPOSITORY'),
        lifecycleRunId: requiredEnvironment('GITHUB_RUN_ID'),
        lifecycleRunAttempt: requiredEnvironment('GITHUB_RUN_ATTEMPT'),
        arch,
        sourceCommit: provenance.candidate.headSha,
        artifacts: before,
      }),
    )
    .digest('hex');
  const operatorPublicKeySha256 = requiredEnvironment(
    'TALKING_QUILL_MACOS_OPERATOR_PUBLIC_KEY_SHA256',
  );
  if (!/^[0-9a-f]{64}$/u.test(operatorPublicKeySha256)) {
    throw new Error('Invalid operator public key pin');
  }
  const checkpoints = [];
  const auto = (id, observation) => checkpoints.push(automaticCheckpoint(id, observation));
  const manual = async (id, instructions) =>
    checkpoints.push(
      await awaitManualCheckpoint(
        checkpointRoot,
        id,
        instructions,
        manualTimeout,
        sessionBindingSha256,
        operatorPublicKeySha256,
      ),
    );

  let dmgMount = null;
  try {
    const zipApp = extractZip(candidateZip, resolve(workRoot, 'candidate-zip'));
    const baselineApp = extractZip(baselineZip, resolve(workRoot, 'baseline-zip'));
    const mounted = mountDmg(candidateDmg);
    dmgMount = mounted.mount;
    const dmgApp = onlyApplication(mounted.mount);
    const candidateTree = treeSha256(zipApp);
    const dmgTree = treeSha256(dmgApp);
    if (candidateTree !== dmgTree)
      throw new Error('Candidate DMG and ZIP contain different application trees');
    const baselineTree = treeSha256(baselineApp);
    const candidateIdentity = signingIdentity(zipApp, signingMode, workRoot, 'candidate');
    const baselineIdentity = signingIdentity(baselineApp, signingMode, workRoot, 'baseline');
    enforceIdentity(signingMode, candidateIdentity, baselineIdentity);
    const candidatePolicy = policy(zipApp);
    const baselinePolicy = policy(baselineApp);
    if (candidatePolicy.installationIdentityDigest !== baselinePolicy.installationIdentityDigest) {
      throw new Error(
        'Accepted predecessor and candidate do not preserve local installation identity',
      );
    }
    const candidateParts = roleHashes(zipApp);
    const baselineParts = roleHashes(baselineApp);
    const candidateRelease = releaseMetadata(zipApp);
    const baselineRelease = releaseMetadata(baselineApp);
    const predecessor = candidateRelease.predecessor;
    if (
      predecessor?.platform !== 'mac' ||
      predecessor.architecture !== arch ||
      predecessor.version !== baselineRelease.version ||
      predecessor.releaseBuildDigest !== baselineRelease.releaseBuildDigest ||
      predecessor.gatewaySha256 !== baselineParts.gateway ||
      predecessor.ownerSha256 !== baselineParts.owner ||
      baselineRelease.architecture !== arch
    )
      throw new Error('Tested macOS predecessor does not match candidate release metadata');

    removeInstalledApp();
    installApplication(dmgApp, { preserveQuarantine: true });
    if (treeSha256(APP) !== dmgTree) throw new Error('DMG installation changed application bytes');
    auto('dmg-install-exact-app', { artifact: before.candidateDmg.sha256Before, appTree: dmgTree });
    run('xattr', ['-w', 'com.apple.quarantine', '0081;00000000;TalkingQuillR11;', APP], [0]);
    const gatekeeper = run('spctl', ['--assess', '--type', 'execute', '--verbose=2', APP], [1, 3]);
    if (
      !/(?:rejected|unnotarized|no usable signature)/iu.test(
        `${gatekeeper.stdout}\n${gatekeeper.stderr}`,
      )
    ) {
      throw new Error('Gatekeeper did not report the expected local-signature rejection');
    }
    await manual(
      'local-install-anyway',
      signingMode === 'adhoc'
        ? 'Use Finder Privacy & Security/Open Anyway for this ad-hoc exact DMG installation. Confirm that no replacement or re-signing occurred.'
        : 'Use Finder Open or Privacy & Security/Open Anyway for this locally self-signed exact DMG installation. Confirm the pinned local certificate.',
    );
    run('open', ['-a', APP], [0]);
    await delay(2_000);
    if (treeSha256(APP) !== dmgTree)
      throw new Error('Install-anyway flow changed application bytes');
    auto('stable-local-identity', candidateIdentity);
    probeInstalled(candidatePolicy.releaseBuildDigest);
    requireBridgeStatus('1');
    run(bridgePath(), ['unregister'], [0]);
    requireBridgeStatus('0', '3');
    probeInstalled(candidatePolicy.releaseBuildDigest);
    requireBridgeStatus('1');
    auto('smappservice-register-launch-unregister', {
      bridge: sha256File(bridgePath()),
      owner: candidateParts.owner,
    });
    auto('keychain-gateway-allow', { probeBuild: candidatePolicy.releaseBuildDigest });
    const denial = run(
      `${APP}/Contents/MacOS/Talking Quill`,
      ['--macos-owner-acl-denial-test'],
      [77],
    );
    auto('keychain-electron-deny', { exitStatus: denial.status });
    await manual(
      'keychain-owner-allow',
      'Confirm the launched LoginItem reads both fixed Keychain items with authentication UI disabled.',
    );
    await manual(
      'tcc-grant-capture',
      'Grant Accessibility and Input Monitoring to the nested owner, relaunch, and confirm physical capture.',
    );
    await manual(
      'tcc-revoke-fail-closed',
      'Revoke TCC while active and confirm new capture closes while owned keys drain/replay.',
    );
    await manual(
      'tcc-regrant-recovery',
      'Regrant TCC, relaunch, and confirm capture recovers for this exact identity.',
    );

    // Rebase onto the accepted predecessor without rebuilding either artifact.
    run(bridgePath(), ['unregister'], [0, 1]);
    removeInstalledApp();
    installApplication(baselineApp);
    probeInstalled(baselinePolicy.releaseBuildDigest);
    await manual(
      'baseline-tcc-grant-capture',
      'Grant/regrant TCC to the exact accepted predecessor owner and prove physical capture is active before the held-key update.',
    );
    await manual(
      'physical-shortcut-held',
      'Press and continue holding the configured physical shortcut. Submit only while it remains held.',
    );
    const heldTree = treeSha256(APP);
    const updater = spawn(
      'open',
      ['-W', '-a', APP, '--args', `--update-local-owner=${candidateZip}`],
      {
        stdio: 'ignore',
      },
    );
    await delay(20_000); // exceeds the selected 15-second neutral grace
    if (treeSha256(APP) !== heldTree) {
      updater.kill('SIGTERM');
      throw new Error('Finalizer replaced the app while a physical shortcut remained held');
    }
    auto('held-key-finalizer-postponed', {
      predecessorTree: heldTree,
      observedMilliseconds: 20_000,
    });
    await manual(
      'physical-shortcut-replay',
      'Release the held shortcut and confirm every suppressed key is replayed once before update proceeds.',
    );
    await waitForExit(updater, 180_000);
    await waitForBuild(candidatePolicy.releaseBuildDigest, 180_000);
    const updatedTree = treeSha256(APP);
    if (updatedTree !== candidateTree)
      throw new Error('ZIP update did not install the exact candidate app tree');
    auto('zip-update-exact-app', {
      artifact: before.candidateZip.sha256Before,
      appTree: updatedTree,
    });
    await manual(
      'candidate-tcc-post-update-recovery',
      signingMode === 'adhoc'
        ? 'Remove stale TCC identity, grant the changed ad-hoc candidate owner, relaunch, and prove capture recovers.'
        : 'Confirm the stable self-signed candidate retains or recovers TCC capture after exact ZIP update.',
    );
    await manual(
      'physical-option-command-replay',
      'Exercise left/right Option and Command held/release paths and confirm exact replay with no stuck modifier.',
    );
    await manual(
      'physical-paste',
      'Complete one real paste insertion into a non-Electron target and confirm target/text correctness.',
    );

    crashOwner();
    probeInstalled(candidatePolicy.releaseBuildDigest);
    auto('owner-crash-recovery', { build: candidatePolicy.releaseBuildDigest });
    run('open', ['-W', '-a', APP, '--args', `--rollback-local-owner=${baselineZip}`], [0], 240_000);
    await waitForBuild(baselinePolicy.releaseBuildDigest, 180_000);
    const rollbackTree = treeSha256(APP);
    if (rollbackTree !== baselineTree)
      throw new Error('Controlled rollback did not restore exact baseline bytes');
    auto('controlled-rollback', {
      artifact: before.baselineZip.sha256Before,
      appTree: rollbackTree,
    });

    // Drag-to-Trash is separately observed from controlled uninstall.
    const trash = resolve(process.env.HOME, '.Trash', `Talking Quill R11 ${process.pid}.app`);
    rmSync(trash, { recursive: true, force: true });
    renameSync(APP, trash);
    await manual(
      'drag-to-trash-cleanup',
      'Confirm LoginItem unregistered, owner exited after drain, fixed Keychain items are absent, and no removal poison/runtime socket remains.',
    );
    rmSync(trash, { recursive: true, force: true });
    installApplication(zipApp);
    probeInstalled(candidatePolicy.releaseBuildDigest);
    run('open', ['-W', '-a', APP, '--args', '--uninstall-local-owner'], [0], 240_000);
    await waitAbsent(APP, 180_000);
    assertCleanupAbsent();
    await manual(
      'controlled-uninstall-cleanup',
      'Confirm SMAppService is unregistered, owner/gateway are absent, fixed Keychain items are absent, and TCC shows no running owner after controlled uninstall.',
    );
    await manual(
      'persisted-identity-continuity',
      'Confirm the same non-secret local installation identity was observed across predecessor install, candidate update, crash recovery, rollback, reinstall, and uninstall.',
    );

    const after = artifactSnapshot({ candidateDmg, candidateZip, baselineZip });
    for (const role of Object.keys(before)) {
      if (
        before[role].sha256Before !== after[role].sha256Before ||
        before[role].bytes !== after[role].bytes
      ) {
        throw new Error(`Input artifact changed during execution: ${role}`);
      }
      before[role].sha256After = after[role].sha256Before;
    }
    const orderedCheckpoints = MACOS_R11_CHECKPOINTS.map((id) => {
      const match = checkpoints.find((checkpoint) => checkpoint.id === id);
      if (!match) throw new Error(`Missing checkpoint: ${id}`);
      return match;
    });
    writeMacosR11Evidence(
      evidencePath,
      {
        schemaVersion: 1,
        kind: 'macos-r11-installed-lifecycle',
        platform: 'mac',
        arch,
        sourceCommit: provenance.candidate.headSha,
        candidateTag: requiredEnvironment('TALKING_QUILL_RELEASE_TAG'),
        sessionBindingSha256,
        releaseRun: { id: provenance.candidate.runId, attempt: provenance.candidate.runAttempt },
        lifecycleRun: {
          id: requiredEnvironment('GITHUB_RUN_ID'),
          attempt: requiredEnvironment('GITHUB_RUN_ATTEMPT'),
        },
        runner: {
          os: requiredEnvironment('RUNNER_OS'),
          arch: requiredEnvironment('RUNNER_ARCH'),
          name: requiredEnvironment('RUNNER_NAME'),
        },
        signing: {
          mode: signingMode,
          candidateRequirement: candidateIdentity.requirement,
          baselineRequirement: baselineIdentity.requirement,
          candidateLeafSha256: candidateIdentity.leafSha256,
          baselineLeafSha256: baselineIdentity.leafSha256,
          candidateCdHash: candidateIdentity.cdHash,
          baselineCdHash: baselineIdentity.cdHash,
        },
        artifacts: before,
        predecessor: {
          ...predecessor,
          runId: provenance.baseline.runId,
          runAttempt: provenance.baseline.runAttempt,
          headSha: provenance.baseline.headSha,
          artifactName: before.baselineZip.name,
          artifactSha256: before.baselineZip.sha256Before,
        },
        installed: {
          dmgAppTreeSha256: dmgTree,
          zipAppTreeSha256: candidateTree,
          updatedAppTreeSha256: updatedTree,
          rollbackAppTreeSha256: rollbackTree,
          candidateGatewaySha256: candidateParts.gateway,
          candidateOwnerSha256: candidateParts.owner,
          baselineGatewaySha256: baselineParts.gateway,
          baselineOwnerSha256: baselineParts.owner,
          installationIdSha256: candidatePolicy.installationIdentityDigest,
        },
        checkpoints: orderedCheckpoints,
        result: 'passed',
      },
      {
        arch,
        sourceCommit: provenance.candidate.headSha,
        sessionBindingSha256,
        operatorPublicKeySha256,
        artifactSha256: {
          candidateDmg: before.candidateDmg.sha256Before,
          candidateZip: before.candidateZip.sha256Before,
          baselineZip: before.baselineZip.sha256Before,
        },
      },
    );
  } finally {
    if (dmgMount) run('hdiutil', ['detach', dmgMount, '-force'], [0, 1]);
  }
}

function submitCheckpoint(options) {
  const requestPath = existing(required(options, 'request'));
  const privateKeyPath = existing(required(options, 'private-key'));
  const request = JSON.parse(readFileSync(requestPath, 'utf8'));
  if (
    !MACOS_R11_CHECKPOINTS.includes(request.id) ||
    !/^[0-9a-f]{64}$/u.test(request.challenge) ||
    !/^[0-9a-f]{64}$/u.test(request.sessionBindingSha256)
  ) {
    throw new Error('Invalid checkpoint request');
  }
  const challengeSha256 = createHash('sha256').update(request.challenge).digest('hex');
  const statement = {
    id: request.id,
    challengeSha256,
    sessionBindingSha256: request.sessionBindingSha256,
    result: 'passed',
    observedAt: Date.now(),
    operator: userInfo().username,
    host: hostname(),
  };
  const payload = Buffer.from(canonicalJson(statement));
  const privateKey = createPrivateKey(readFileSync(privateKeyPath));
  if (privateKey.asymmetricKeyType !== 'ed25519') {
    throw new Error('Checkpoint private key must be Ed25519');
  }
  const publicDer = createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  const attestation = {
    algorithm: 'ed25519',
    publicKeySpkiBase64: publicDer.toString('base64'),
    publicKeySha256: createHash('sha256').update(publicDer).digest('hex'),
    payloadBase64: payload.toString('base64'),
    signatureBase64: sign(null, payload, privateKey).toString('base64'),
  };
  const responsePath = `${requestPath}.passed.json`;
  writeFileSync(responsePath, `${JSON.stringify({ statement, attestation }, null, 2)}\n`, {
    flag: 'wx',
    mode: 0o600,
  });
  process.stdout.write(`Recorded signed staffed checkpoint ${request.id}\n`);
}

async function awaitManualCheckpoint(
  root,
  id,
  instructions,
  timeout,
  sessionBindingSha256,
  operatorPublicKeySha256,
) {
  const challenge = randomBytes(32).toString('hex');
  const request = resolve(
    root,
    `${String(MACOS_R11_CHECKPOINTS.indexOf(id) + 1).padStart(2, '0')}-${id}.request.json`,
  );
  writeFileSync(
    request,
    `${JSON.stringify(
      { id, challenge, sessionBindingSha256, instructions, requestedAt: Date.now() },
      null,
      2,
    )}\n`,
    { flag: 'wx', mode: 0o600 },
  );
  process.stdout.write(
    `MANUAL CHECKPOINT ${id}: ${instructions}\nSubmit from the staffed operator account with its protected key:\nnode scripts/macos-r11-installed-lifecycle.mjs --checkpoint --request ${JSON.stringify(request)} --private-key /protected/operator-ed25519.pem\n`,
  );
  const response = `${request}.passed.json`;
  const deadline = Date.now() + timeout;
  while (!existsSync(response) && Date.now() < deadline) await delay(1_000);
  if (!existsSync(response)) throw new Error(`Timed out waiting for staffed checkpoint: ${id}`);
  const value = JSON.parse(readFileSync(response, 'utf8'));
  const challengeSha256 = createHash('sha256').update(challenge).digest('hex');
  const publicDer = Buffer.from(value?.attestation?.publicKeySpkiBase64 ?? '', 'base64');
  let publicKey;
  try {
    publicKey = createPublicKey({ key: publicDer, format: 'der', type: 'spki' });
  } catch {
    throw new Error(`Invalid staffed checkpoint key: ${id}`);
  }
  const payload = Buffer.from(value?.attestation?.payloadBase64 ?? '', 'base64');
  const signature = Buffer.from(value?.attestation?.signatureBase64 ?? '', 'base64');
  if (
    value?.statement?.id !== id ||
    value.statement.challengeSha256 !== challengeSha256 ||
    value.statement.sessionBindingSha256 !== sessionBindingSha256 ||
    value.statement.result !== 'passed' ||
    !Number.isSafeInteger(value.statement.observedAt) ||
    value.attestation.algorithm !== 'ed25519' ||
    createHash('sha256').update(publicDer).digest('hex') !== operatorPublicKeySha256 ||
    canonicalJson(value.statement) !== payload.toString('utf8') ||
    !verify(null, payload, publicKey, signature)
  ) {
    throw new Error(`Invalid staffed checkpoint response: ${id}`);
  }
  return {
    id,
    method: 'staffed-manual',
    challengeSha256,
    observedAt: value.statement.observedAt,
    result: 'passed',
    attestation: value.attestation,
  };
}
function automaticCheckpoint(id, observation) {
  return checkpoint(id, 'automated', Date.now(), observation);
}
function checkpoint(id, method, observedAt, observation) {
  return {
    id,
    method,
    challengeSha256: createHash('sha256').update(JSON.stringify(observation)).digest('hex'),
    observedAt,
    result: 'passed',
    attestation: null,
  };
}

function artifactSnapshot(paths) {
  return Object.fromEntries(
    Object.entries(paths).map(([role, path]) => [
      role,
      {
        name: basename(path),
        bytes: statSync(path).size,
        sha256Before: sha256File(path),
        sha256After: sha256File(path),
      },
    ]),
  );
}
function verifyProvenance(value, arch, baselineHash) {
  if (
    value?.schemaVersion !== 1 ||
    value?.kind !== 'macos-r11-authenticated-inputs' ||
    value?.repository !== process.env.GITHUB_REPOSITORY ||
    value?.workflowPath !== '.github/workflows/release-unsigned.yml' ||
    value?.arch !== arch ||
    value?.candidate?.headSha !== process.env.TALKING_QUILL_RELEASE_COMMIT ||
    value?.candidate?.runId !== process.env.TALKING_QUILL_RELEASE_RUN_ID ||
    value?.candidate?.runAttempt !== process.env.TALKING_QUILL_RELEASE_RUN_ATTEMPT ||
    value?.baseline?.acceptedZipSha256 !== baselineHash
  ) {
    throw new Error('Authenticated macOS provenance does not match exact lifecycle inputs');
  }
}
function extractZip(zip, root) {
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  run('ditto', ['-x', '-k', zip, root], [0]);
  return onlyApplication(root);
}
function mountDmg(dmg) {
  const result = run('hdiutil', ['attach', '-nobrowse', '-readonly', dmg], [0]);
  const line = result.stdout
    .trim()
    .split(/\r?\n/u)
    .findLast((entry) => entry.includes('/Volumes/'));
  const match = line?.match(/(\/Volumes\/.*)$/u);
  if (!match) throw new Error('Could not identify mounted DMG volume');
  return { mount: match[1] };
}
function onlyApplication(root) {
  const apps = readdirSync(root).filter((name) => name.endsWith('.app'));
  if (apps.length !== 1) throw new Error(`Expected exactly one application under ${root}`);
  return resolve(root, apps[0]);
}
function installApplication(source, options = {}) {
  removeInstalledApp();
  run('ditto', [source, APP], [0]);
  if (options.preserveQuarantine && !existsSync(APP)) {
    throw new Error('Quarantined application installation failed');
  }
}
function removeInstalledApp() {
  if (existsSync(bridgePath())) run(bridgePath(), ['unregister'], [0, 1, 3]);
  rmSync(APP, { recursive: true, force: true });
}
function policy(app) {
  const value = JSON.parse(readFileSync(resolve(app, POLICY_RELATIVE), 'utf8'));
  if (
    !/^[0-9a-f]{64}$/u.test(value.releaseBuildDigest) ||
    !/^[0-9a-f]{64}$/u.test(value.installationIdentityDigest)
  ) {
    throw new Error('Invalid packaged owner policy');
  }
  return value;
}
function releaseMetadata(app) {
  const value = JSON.parse(readFileSync(resolve(app, RELEASE_METADATA_RELATIVE), 'utf8'));
  if (
    value?.schemaVersion !== 1 ||
    value?.kind !== 'talking-quill-local-owner-release' ||
    value?.platform !== 'mac' ||
    !/^[0-9a-f]{64}$/u.test(value?.releaseBuildDigest ?? '')
  )
    throw new Error('Invalid packaged release metadata');
  return value;
}
function roleHashes(app) {
  return {
    gateway: sha256File(resolve(app, GATEWAY_RELATIVE)),
    owner: sha256File(resolve(app, OWNER_RELATIVE)),
  };
}
function signingIdentity(app, mode, workRoot, label) {
  const owner = resolve(app, OWNER_RELATIVE);
  const requirementOutput = run('codesign', ['-d', '-r-', owner], [0]).stderr;
  const requirement = requirementOutput.split('designated => ')[1]?.trim();
  const detail = run('codesign', ['-dvvv', owner], [0]).stderr;
  const cdHash = /^CDHash=([0-9a-f]+)$/imu.exec(detail)?.[1]?.toLowerCase();
  if (!requirement || !cdHash) throw new Error(`Could not read ${label} code identity`);
  const isAdhoc = /^Signature=adhoc$/imu.test(detail);
  if ((mode === 'adhoc') !== isAdhoc) {
    throw new Error(`${label} signing mode does not match the requested lifecycle mode`);
  }
  let leafSha256 = null;
  if (mode === 'self-signed') {
    const prefix = resolve(workRoot, `${label}-certificate`);
    run('codesign', ['-d', '--extract-certificates', prefix, owner], [0]);
    const certificate = `${prefix}0`;
    leafSha256 = sha256File(certificate);
    const pem = resolve(workRoot, `${label}-certificate.pem`);
    run('openssl', ['x509', '-inform', 'DER', '-in', certificate, '-out', pem], [0]);
    const names = run(
      'openssl',
      ['x509', '-in', pem, '-noout', '-subject', '-issuer', '-nameopt', 'RFC2253'],
      [0],
    )
      .stdout.trim()
      .split(/\r?\n/u)
      .map((line) => line.replace(/^[^=]+=/u, ''));
    if (names.length !== 2 || names[0] !== names[1]) {
      throw new Error(`${label} certificate issuer and subject differ`);
    }
    run('openssl', ['verify', '-check_ss_sig', '-CAfile', pem, pem], [0]);
  }
  return { requirement, cdHash, leafSha256 };
}
function enforceIdentity(mode, candidate, baseline) {
  if (
    mode === 'self-signed' &&
    (candidate.leafSha256 !== baseline.leafSha256 || candidate.requirement !== baseline.requirement)
  ) {
    throw new Error(
      'Accepted baseline and candidate do not preserve the local self-signed identity',
    );
  }
  if (mode === 'adhoc' && (candidate.leafSha256 !== null || baseline.leafSha256 !== null)) {
    throw new Error('Ad-hoc mode unexpectedly extracted a certificate');
  }
}
function probeInstalled(build) {
  run(resolve(APP, GATEWAY_RELATIVE), ['--macos-owner-validate-install'], [0]);
  run(resolve(APP, GATEWAY_RELATIVE), ['--macos-owner-probe', build], [0]);
}
function bridgePath() {
  return resolve(APP, BRIDGE_RELATIVE);
}
function requireBridgeStatus(...expected) {
  const status = run(bridgePath(), ['status'], [0]).stdout.trim();
  if (!expected.includes(status)) {
    throw new Error(`Unexpected SMAppService status: ${status}`);
  }
}
function crashOwner() {
  run('pkill', ['-KILL', '-f', `${APP}/${OWNER_RELATIVE}`], [0]);
}
async function waitForBuild(build, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      if (policy(APP).releaseBuildDigest === build) {
        probeInstalled(build);
        return;
      }
    } catch {
      // Replacement can make the bundle temporarily absent or incomplete.
    }
    await delay(1_000);
  }
  throw new Error(`Timed out waiting for installed build ${build}`);
}
async function waitAbsent(path, timeout) {
  const deadline = Date.now() + timeout;
  while (existsSync(path) && Date.now() < deadline) await delay(1_000);
  if (existsSync(path)) throw new Error(`Timed out waiting for removal: ${path}`);
}
function assertCleanupAbsent() {
  const root = resolve(process.env.HOME, 'Library/Application Support/Talking Quill/KeyboardOwner');
  for (const relativePath of ['removal-required-v1', 'run-v1']) {
    const path = resolve(root, relativePath);
    if (existsSync(path) && (lstatSync(path).isFile() || readdirSync(path).length > 0)) {
      throw new Error(`Committed uninstall left owner residue: ${relativePath}`);
    }
  }
  const maintenance = resolve(root, 'maintenance-v1');
  if (existsSync(maintenance) && readdirSync(maintenance).length > 0) {
    throw new Error('Committed uninstall left maintenance journals');
  }
}
function treeSha256(root) {
  const hash = createHash('sha256');
  const visit = (directory) => {
    for (const name of readdirSync(directory).sort()) {
      const path = resolve(directory, name);
      const item = lstatSync(path);
      const rel = relative(root, path).split(sep).join('/');
      if (item.isSymbolicLink()) hash.update(`L\0${rel}\0${readlinkSync(path)}\0`);
      else if (item.isDirectory()) {
        hash.update(`D\0${rel}\0`);
        visit(path);
      } else if (item.isFile())
        hash.update(`F\0${rel}\0${item.mode & 0o777}\0`).update(readFileSync(path));
      else throw new Error(`Unsupported application entry: ${rel}`);
    }
  };
  visit(root);
  return hash.digest('hex');
}
function run(command, args, statuses, timeout = 300_000) {
  const result = spawnSync(command, args, { encoding: 'utf8', timeout, env: process.env });
  if (!statuses.includes(result.status)) {
    throw new Error(`${command} ${args.join(' ')} exited ${result.status}: ${result.stderr}`);
  }
  return result;
}
async function waitForExit(child, timeout) {
  if (child.exitCode !== null) {
    if (child.exitCode !== 0) throw new Error(`Updater exited ${child.exitCode}`);
    return;
  }
  await Promise.race([
    new Promise((accept, reject) =>
      child.once('exit', (code) =>
        code === 0 ? accept() : reject(new Error(`Updater exited ${code}`)),
      ),
    ),
    delay(timeout).then(() => {
      child.kill('SIGTERM');
      throw new Error('Updater timed out');
    }),
  ]);
}
function assertWorkRoot(path) {
  const repositoryTmp = resolve('tmp');
  if (path !== repositoryTmp && !path.startsWith(`${repositoryTmp}${sep}`)) {
    throw new Error('Lifecycle work root must be beneath repository tmp/');
  }
}
function existing(path) {
  const absolute = resolve(path);
  if (!existsSync(absolute) || !statSync(absolute).isFile())
    throw new Error(`Missing regular file: ${absolute}`);
  return absolute;
}
function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}
function parse(args) {
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    if (!args[index]?.startsWith('--') || args[index + 1] === undefined)
      throw new Error('Expected --name value');
    const name = args[index].slice(2);
    if (values.has(name)) throw new Error(`Duplicate --${name}`);
    values.set(name, args[index + 1]);
  }
  return values;
}
function required(values, name) {
  const value = values.get(name);
  if (!value) throw new Error(`Missing --${name}`);
  return value;
}
function delay(milliseconds) {
  return new Promise((accept) => setTimeout(accept, milliseconds));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
