import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';

const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9.-]{0,254}$/u;
const TEAM_IDENTIFIER = /^[A-Z0-9]{10}$/u;
const MAX_OUTPUT_BYTES = 64 * 1024;
const MAX_REQUIREMENT_BYTES = 8 * 1024;

export function inspectMacosOuterIdentity(path) {
  const result = spawnSync('/usr/bin/codesign', ['-d', '-r-', '--verbose=4', path], {
    encoding: 'utf8',
    maxBuffer: MAX_OUTPUT_BYTES,
  });
  if (result.status !== 0 || result.error !== undefined) {
    throw new Error(`codesign outer identity inspection failed: ${path}`);
  }
  const output = `${result.stdout}${result.stderr}`;
  if (/^Signature=adhoc$/mu.test(output)) return parseMacosCodesignIdentity(output, null);
  const temporary = mkdtempSync(join(process.cwd(), 'tmp/macos-outer-certificate-'));
  try {
    const prefix = join(temporary, 'leaf');
    const certificate = spawnSync(
      '/usr/bin/codesign',
      ['-d', '--extract-certificates', prefix, path],
      { encoding: 'utf8', maxBuffer: MAX_OUTPUT_BYTES },
    );
    if (certificate.status !== 0 || certificate.error !== undefined) {
      throw new Error(`codesign outer certificate extraction failed: ${path}`);
    }
    const leafCertificateSha256 = createHash('sha256')
      .update(readFileSync(`${prefix}0`))
      .digest('hex');
    return parseMacosCodesignIdentity(output, leafCertificateSha256);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

export function parseMacosCodesignIdentity(output, leafCertificateSha256 = null) {
  if (
    typeof output !== 'string' ||
    output.includes('\0') ||
    Buffer.byteLength(output) > MAX_OUTPUT_BYTES
  ) {
    throw new Error('codesign outer identity output is invalid');
  }
  const lines = output.split(/\r?\n/u);
  const identifier = one(lines, /^Identifier=(.+)$/u, 'identifier');
  const teamValue = one(lines, /^TeamIdentifier=(.+)$/u, 'team identifier');
  const requirement = one(lines, /^designated => (.+)$/u, 'designated requirement');
  const signature = one(lines, /^Signature(?:=| size=)(.+)$/u, 'signature');
  if (!IDENTIFIER.test(identifier) || Buffer.byteLength(requirement) > MAX_REQUIREMENT_BYTES) {
    throw new Error('codesign outer identity fields are invalid');
  }
  const teamIdentifier = teamValue === 'not set' ? null : teamValue;
  if (teamIdentifier !== null && !TEAM_IDENTIFIER.test(teamIdentifier)) {
    throw new Error('codesign outer team identifier is invalid');
  }
  const mode = signature === 'adhoc' ? 'adhoc' : 'certificate';
  if (
    (mode === 'certificate' && !/^[0-9a-f]{64}$/u.test(leafCertificateSha256 ?? '')) ||
    (mode === 'adhoc' && leafCertificateSha256 !== null)
  ) {
    throw new Error('codesign outer certificate identity is invalid');
  }
  return Object.freeze({
    mode,
    leafCertificateSha256,
    identifier,
    teamIdentifier,
    designatedRequirement: requirement,
    designatedRequirementSha256: createHash('sha256').update(requirement).digest('hex'),
  });
}

function one(lines, pattern, name) {
  const values = lines.flatMap((line) => {
    const match = pattern.exec(line);
    return match?.[1] === undefined ? [] : [match[1]];
  });
  if (values.length !== 1 || values[0].length === 0) {
    throw new Error(`codesign outer ${name} must appear exactly once`);
  }
  return values[0];
}
