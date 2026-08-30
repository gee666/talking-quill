import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { parseMacosCodesignIdentity } from '../../scripts/macos-outer-identity.mjs';

function output(
  options: {
    requirement?: string;
    team?: string;
    signature?: string;
    identifier?: string;
  } = {},
): string {
  return [
    'Executable=/Applications/Talking Quill.app/Contents/MacOS/Talking Quill',
    `Identifier=${options.identifier ?? 'com.talkingquill.app'}`,
    options.signature === 'adhoc'
      ? 'Signature=adhoc'
      : `Signature size=${options.signature ?? '4789'}`,
    `TeamIdentifier=${options.team ?? 'not set'}`,
    `designated => ${options.requirement ?? 'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"'}`,
    '',
  ].join('\n');
}

describe('macOS outer code identity inspection', () => {
  it('retains certificate identity, nullable local team, and exact designated requirement', () => {
    const leafCertificateSha256 = '11'.repeat(32);
    const parsed = parseMacosCodesignIdentity(output(), leafCertificateSha256);
    expect(parsed).toEqual({
      mode: 'certificate',
      leafCertificateSha256,
      identifier: 'com.talkingquill.app',
      teamIdentifier: null,
      designatedRequirement:
        'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"',
      designatedRequirementSha256: createHash('sha256')
        .update(
          'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"',
        )
        .digest('hex'),
    });
  });

  it('retains a Developer ID team identifier', () => {
    expect(
      parseMacosCodesignIdentity(output({ team: 'TEAMID1234' }), '22'.repeat(32)).teamIdentifier,
    ).toBe('TEAMID1234');
  });

  it.each([
    output({ signature: 'adhoc' }),
    output({ identifier: '../attacker' }),
    output({ team: 'not-a-team' }),
    `${output()}Identifier=attacker.example\n`,
    output().replace('designated => ', 'designated lookalike => '),
  ])('classifies ad-hoc and rejects malformed or ambiguous identity output %#', (value) => {
    if (value.includes('Signature=adhoc')) {
      expect(parseMacosCodesignIdentity(value, null).mode).toBe('adhoc');
    } else {
      expect(() => parseMacosCodesignIdentity(value, '33'.repeat(32))).toThrow();
    }
  });
});
