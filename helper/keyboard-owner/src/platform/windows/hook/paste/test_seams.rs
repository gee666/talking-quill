//! Debug-only synchronization points for native paste acceptance.
use super::*;

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
pub(in crate::platform::windows::hook) fn pause_after_paste_admission(deadline: Instant) {
    let (Some(arm), Some(admitted), Some(release)) = (
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_ARM"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_ADMITTED"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_RELEASE"),
    ) else {
        return;
    };
    let arm = std::path::PathBuf::from(arm);
    if !arm.try_exists().unwrap_or(false) {
        return;
    }
    let admitted = std::path::PathBuf::from(admitted);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::remove_file(arm);
    let _ = std::fs::write(&admitted, b"paste admitted\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
pub(in crate::platform::windows::hook) fn pause_after_valid_clipboard_sample() {
    let (Some(arm), Some(sampled), Some(release)) = (
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_ARM"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_SAMPLED"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_RELEASE"),
    ) else {
        return;
    };
    let arm = std::path::PathBuf::from(arm);
    if !arm.try_exists().unwrap_or(false) {
        return;
    }
    let sampled = std::path::PathBuf::from(sampled);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::remove_file(arm);
    let _ = std::fs::write(&sampled, b"valid clipboard hash sampled\n");
    let seam_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < seam_deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}
