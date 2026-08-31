import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('native Windows uninstall profile policy', () => {
  it('preserves the signed-in profile by default', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('delete_profile');
    expect(setup).toContain('FOLDERID_LocalAppData');
    expect(setup).toContain('remove_plain_tree(&controller_paths.profile)');
  });

  it('does not accept a command-line data deletion authority', async () => {
    const setup = await readFile('installer/windows-setup/src/windows.rs', 'utf8');
    expect(setup).toContain('arguments.len() == 1 && arguments[0] == "/S"');
    expect(setup).not.toContain('--delete-user-data');
  });
});
