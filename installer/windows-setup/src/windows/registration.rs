//! Installation paths, maintenance images, and application registration.
use super::*;

mod paths;
pub(super) use paths::*;

mod maintenance_image;
pub(super) use maintenance_image::*;

mod finalizer;
pub(super) use finalizer::*;

mod registry;
pub(super) use registry::*;

pub(super) const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill";
pub(super) const APP_PATH_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe";

pub(super) fn register_installed_uninstall(paths: &Paths) -> Result<()> {
    ensure_maintenance_uninstaller(paths)?;
    let manifest = paths
        .install
        .join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&manifest)?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest).map_err(io_failure)?)
        .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
    let version = value
        .get("version")
        .and_then(|item| item.as_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Installed release version is invalid."))?;
    register_uninstall(paths, version)?;
    register_app_path(paths)?;
    reclaim_stale_maintenance_uninstallers(paths)
}
