import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

const source = readFileSync('installer/windows-setup/src/windows.rs', 'utf8');
const cargo = readFileSync('installer/windows-setup/Cargo.toml', 'utf8');

function index(text: string): number {
  const value = source.indexOf(text);
  expect(value, `missing ${text}`).toBeGreaterThan(-1);
  return value;
}

describe('Windows schema-2 stale coordination cleanup', () => {
  it('is a feature-gated authenticated elevated operation', () => {
    expect(cargo).toContain('stale-schema2-cleanup = []');
    expect(source).toContain('#[cfg(feature = "stale-schema2-cleanup")]\n    CleanStaleSchema2');
    expect(source).toContain('WorkerChannel::connect_and_authenticate(&current, None)?');
    expect(index('requested_action == Some(Action::CleanStaleSchema2)')).toBeGreaterThan(
      index('WorkerChannel::connect_and_authenticate(&current, None)?'),
    );
    expect(source).toContain('if !token_is_elevated()?');
  });

  it('pins the exact unpublished synthetic record bytes and hash', () => {
    const literal = source.match(/const SYNTHETIC_SCHEMA2_BYTES: &\[u8\] = br#"(.+?)"#;/s)?.[1];
    expect(literal).toBeDefined();
    expect(JSON.parse(literal ?? '')).toMatchObject({
      schemaVersion: 2,
      generation: '78bd88811b14faf1e11ba59620088aa0',
      request: '--windows-update-bootstrap-v2=dGVzdA==',
      sourceVersion: '0.0.69',
      targetVersion: '0.0.70',
      phase: 'armed',
    });
    expect(
      createHash('sha256')
        .update(literal ?? '')
        .digest('hex'),
    ).toBe('abb2d6183c58b6ec52e28f6befbe43d949d2eeaf1998122921118272da8f3bad');
    expect(source).toContain('bytes != SYNTHETIC_SCHEMA2_BYTES');
    expect(source).toContain('hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256');
  });

  it('fails closed on owners and takes locks in the production order', () => {
    for (const check of [
      'registry_key_present(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?',
      'registry_key_present(HKEY_LOCAL_MACHINE, APP_PATH_KEY)?',
      '!no_owned_run_values()?',
      '!no_talking_quill_process_except_authenticated_pair()?',
      '!no_owned_service_keys()?',
      'Tasks/TalkingQuillKeyboardAuthority',
      '.Talking Quill.native-transaction-v2.json',
    ]) {
      expect(source).toContain(check);
    }
    expect(index('let legacy = LegacyMutexPair::acquire()?;')).toBeLessThan(
      index('Machine lock is active or cannot be opened exclusively.'),
    );
    expect(source).toContain('share_mode(0)');
    expect(source).toContain('Duration::from_millis(750)');
  });

  it('checks exact inventories, removes through owned-tree identities, and deletes registry last', () => {
    expect(source).toContain('Machine lock fixture inventory is not exact.');
    expect(source).toContain('Escaped schema-2 fixture inventory or cleanup prefix is not exact.');
    expect(source).toContain('remove_owned_tree(&generation, &identity)');
    expect(source).toContain('remove_owned_tree(&lock_directory, &identity)');
    expect(index('remove_owned_tree(&generation, &identity)')).toBeLessThan(
      index('deleting registry publication last'),
    );
    expect(index('remove_owned_tree(&lock_directory, &identity)')).toBeLessThan(
      index('deleting registry publication last'),
    );
    expect(source).toContain('Stale cleanup did not prove zero residue.');
    expect(source).toContain('deleted verified objects; zero residue proven');
  });

  it('limits production reclaim to authenticated fresh installs before paths create state', () => {
    const production = index(
      'package.manifest.package_mode == "fresh" && requested_action == Some(Action::Install)',
    );
    expect(production).toBeGreaterThan(index('TQPKG2 architecture does not match'));
    expect(production).toBeLessThan(index('let mut paths = paths()?;'));
    expect(source).toContain('Cleanup prefix order is invalid.');
  });
});
