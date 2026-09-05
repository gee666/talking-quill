//! Injected native operations for transaction recovery tests.
use super::*;

#[cfg(test)]
pub(in super::super) struct InjectedNativeSystem;

#[cfg(test)]
impl NativeSystemAdapter for InjectedNativeSystem {
    fn register_version(&self, _paths: &Paths, _version: &str) -> Result<()> {
        Ok(())
    }
    fn register_installed(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn unregister_app_path(&self) -> Result<()> {
        Ok(())
    }
    fn unregister_uninstall(&self) -> Result<()> {
        Ok(())
    }
    fn retire_legacy(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn clear_update_recovery(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn clear_relaunch_owner(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(in super::super) fn recover_with_system(
    paths: &Paths,
    update_system_state: bool,
) -> Result<()> {
    if update_system_state {
        recover_with_adapter(paths, &WindowsNativeSystem)
    } else {
        recover_with_adapter(paths, &InjectedNativeSystem)
    }
}
