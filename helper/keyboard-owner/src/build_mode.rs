//! Compile-time keyboard-owner build identity.
//!
//! Local unsigned owner builds enable the native capture gate for an explicitly
//! provisioned personal-use launcher. They do not claim installed bootstrap,
//! authority delivery, or packaging; those remain R8. This mode is independent
//! of Authenticode, Developer ID, notarization, CI evidence, and test-input seams.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerBuildMode {
    SafeDisabled,
    LocalUnsignedOwner,
    TestSeam,
}

impl OwnerBuildMode {
    #[must_use]
    pub const fn capture_enabled_by_default(self) -> bool {
        matches!(self, Self::LocalUnsignedOwner | Self::TestSeam)
    }

    #[must_use]
    pub const fn is_local_unsigned_owner(self) -> bool {
        matches!(self, Self::LocalUnsignedOwner)
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::SafeDisabled => {
                "TALKING_QUILL_KEYBOARD_OWNER=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS"
            }
            Self::LocalUnsignedOwner => {
                "TALKING_QUILL_KEYBOARD_OWNER=DEFAULT_ENABLED_OUT_OF_PROCESS_LOCAL_UNSIGNED"
            }
            Self::TestSeam => "TALKING_QUILL_KEYBOARD_OWNER_TEST_SEAMS=ENABLED_SAFE_NON_PROMOTABLE",
        }
    }
}

#[cfg(any(
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
))]
pub const OWNER_BUILD_MODE: OwnerBuildMode = OwnerBuildMode::TestSeam;

#[cfg(all(
    feature = "local-unsigned-owner",
    not(any(
        feature = "transactional-shortcuts-dev",
        feature = "windows-native-test-input"
    ))
))]
pub const OWNER_BUILD_MODE: OwnerBuildMode = OwnerBuildMode::LocalUnsignedOwner;

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
pub const OWNER_BUILD_MODE: OwnerBuildMode = OwnerBuildMode::SafeDisabled;

pub const OWNER_MODE_MARKER: &str = OWNER_BUILD_MODE.marker();

#[must_use]
pub const fn local_owner_profile() -> &'static str {
    if !OWNER_BUILD_MODE.is_local_unsigned_owner() {
        return "NOT_LOCAL_UNSIGNED_OWNER";
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    return "LOCAL_UNSIGNED_OWNER_WIN_X64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "LOCAL_OWNER_MAC_X64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "LOCAL_OWNER_MAC_ARM64";
    #[cfg(not(any(
        all(windows, target_arch = "x86_64"),
        all(
            target_os = "macos",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )))]
    return "LOCAL_UNSIGNED_OWNER_UNSUPPORTED_TARGET";
}
