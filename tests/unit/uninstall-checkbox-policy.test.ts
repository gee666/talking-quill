import { readRustModule } from '../helpers/rust-source';
import { describe, expect, it } from 'vitest';

describe('native Windows uninstall profile policy', () => {
  it('preserves the signed-in profile by default', async () => {
    const setup = await readRustModule('installer/windows-setup/src/windows.rs');
    expect(setup).toContain('delete_profile');
    expect(setup).toContain('FOLDERID_RoamingAppData');
    expect(setup).toContain('remove_plain_tree(&controller_paths.profile)');
  });

  it('does not accept a command-line data deletion authority', async () => {
    const setup = await readRustModule('installer/windows-setup/src/windows.rs');
    expect(setup).toContain('arguments.len() == 1 && arguments[0] == "/S"');
    expect(setup).not.toContain('--delete-user-data');
  });
});
