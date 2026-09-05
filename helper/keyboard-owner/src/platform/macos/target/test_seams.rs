//! Feature-gated real Secure Input and insertion pause seams.

use super::*;

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) struct TestSecureInputScope {
    pub(super) requested: bool,
    pub(super) enabled: bool,
}

#[cfg(feature = "transactional-shortcuts-dev")]
impl TestSecureInputScope {
    pub(super) fn enable_for_preclaim_seam() -> Self {
        let requested = std::env::var_os("TALKING_QUILL_MACOS_TEST_SECURE_INPUT_PRECLAIM")
            .as_deref()
            == Some(std::ffi::OsStr::new("1"));
        if !requested {
            return Self {
                requested,
                enabled: false,
            };
        }
        // This is the real session API, not a simulated permission flag.
        let enabled = unsafe { ffi::EnableSecureEventInput() == 0 } && secure_input_active();
        Self { requested, enabled }
    }

    pub(super) fn failed_to_enable(&self) -> bool {
        self.requested && !self.enabled
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
impl Drop for TestSecureInputScope {
    fn drop(&mut self) {
        if self.enabled {
            let _ = unsafe { ffi::DisableSecureEventInput() };
        }
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn pause_after_insertion_claim(deadline: Instant) {
    let (Some(paused), Some(release)) = (
        std::env::var_os("TALKING_QUILL_MACOS_TEST_INSERTION_CLAIMED"),
        std::env::var_os("TALKING_QUILL_MACOS_TEST_INSERTION_RELEASE"),
    ) else {
        return;
    };
    let paused = std::path::PathBuf::from(paused);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::write(&paused, b"AX insertion claimed\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn pause_after_range_set(deadline: Instant) {
    let (Some(paused), Some(release)) = (
        std::env::var_os("TALKING_QUILL_MACOS_TEST_RANGE_SET"),
        std::env::var_os("TALKING_QUILL_MACOS_TEST_RANGE_SET_RELEASE"),
    ) else {
        return;
    };
    let paused = std::path::PathBuf::from(paused);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::write(&paused, b"AX exact range set\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}
