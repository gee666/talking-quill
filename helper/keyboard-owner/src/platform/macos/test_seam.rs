//! Feature-gated physical test controls and aggregate reports.
use super::*;

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_SEMANTIC_DRAIN_COMPLETE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_REPLAY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_MOUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_MARKER_ACKS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_OPERATION_GENERATIONS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_OPERATION_OBSERVATIONS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_FORCE_PERMISSION_LOSS: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_TAP_DISABLE_CONFIRMED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_TAP_DRAIN_REENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_KEY_SEEN: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) static TEST_KEY_HELD: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];

#[cfg(feature = "transactional-shortcuts-dev")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(super) enum MacosTestOperationClass {
    Replay = 0,
    Cleanup = 1,
    GapBarrier = 2,
    PasteBarrier = 3,
}

#[cfg(feature = "transactional-shortcuts-dev")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacosPhysicalSeamSnapshot {
    pub replay_sequence: u64,
    pub mouse_sequence: u64,
    pub marker_acknowledgements: u64,
    pub operation_generations: [u64; 4],
    pub operation_observations: [u64; 4],
    pub tap_disable_confirmed: bool,
    pub tap_drain_reenabled: bool,
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn reset_macos_physical_seam() {
    TEST_EVENT_SEQUENCE.store(1, Ordering::Release);
    TEST_REPLAY_SEQUENCE.store(0, Ordering::Release);
    TEST_MOUSE_SEQUENCE.store(0, Ordering::Release);
    TEST_MARKER_ACKS.store(0, Ordering::Release);
    for value in TEST_OPERATION_GENERATIONS
        .iter()
        .chain(TEST_OPERATION_OBSERVATIONS.iter())
    {
        value.store(0, Ordering::Release);
    }
    TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE.store(false, Ordering::Release);
    TEST_SEMANTIC_DRAIN_COMPLETE.store(false, Ordering::Release);
    TEST_FORCE_PERMISSION_LOSS.store(false, Ordering::Release);
    TEST_TAP_DISABLE_CONFIRMED.store(false, Ordering::Release);
    TEST_TAP_DRAIN_REENABLED.store(false, Ordering::Release);
    for value in TEST_KEY_SEEN.iter().chain(TEST_KEY_HELD.iter()) {
        value.store(false, Ordering::Release);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_physical_seam_snapshot() -> MacosPhysicalSeamSnapshot {
    MacosPhysicalSeamSnapshot {
        replay_sequence: TEST_REPLAY_SEQUENCE.load(Ordering::Acquire),
        mouse_sequence: TEST_MOUSE_SEQUENCE.load(Ordering::Acquire),
        marker_acknowledgements: TEST_MARKER_ACKS.load(Ordering::Acquire),
        operation_generations: std::array::from_fn(|index| {
            TEST_OPERATION_GENERATIONS[index].load(Ordering::Acquire)
        }),
        operation_observations: std::array::from_fn(|index| {
            TEST_OPERATION_OBSERVATIONS[index].load(Ordering::Acquire)
        }),
        tap_disable_confirmed: TEST_TAP_DISABLE_CONFIRMED.load(Ordering::Acquire),
        tap_drain_reenabled: TEST_TAP_DRAIN_REENABLED.load(Ordering::Acquire),
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn write_macos_test_report_from_env() {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ProcessTestReport {
        native_resource_teardown_complete: bool,
        semantic_drain_complete: bool,
        #[serde(flatten)]
        seam: MacosPhysicalSeamSnapshot,
    }

    let Some(path) = std::env::var_os("TALKING_QUILL_MACOS_TEST_REPORT") else {
        return;
    };
    let report = serde_json::to_vec(&ProcessTestReport {
        native_resource_teardown_complete: TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE
            .load(Ordering::Acquire),
        semantic_drain_complete: TEST_SEMANTIC_DRAIN_COMPLETE.load(Ordering::Acquire),
        seam: macos_physical_seam_snapshot(),
    })
    .expect("fixed macOS test report serializes");
    if let Err(error) = std::fs::write(path, report) {
        eprintln!("talking-quill-helper: cannot write macOS test report: {error}");
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_physical_key(key_code: u16, key_down: bool, flags: u64) -> bool {
    injection::post_test_physical_key(key_code, key_down, flags)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_physical_mouse_down() -> bool {
    injection::post_test_physical_mouse_down()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_permission_loss() -> bool {
    injection::post_test_permission_loss()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn set_macos_test_secure_input(active: bool) -> bool {
    // This seam calls the real Carbon session API. It is not a simulated flag;
    // callers must always disable it in cleanup.
    // SAFETY: the Carbon session APIs take no pointers or borrowed resources.
    let status = unsafe {
        if active {
            ffi::EnableSecureEventInput()
        } else {
            ffi::DisableSecureEventInput()
        }
    };
    status == 0 && secure_input_active() == active
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_modifier_barrier_contract() -> bool {
    event_tap::test_modifier_barrier_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_permission_disable_recovery_contract() -> bool {
    event_tap::test_permission_disable_recovery_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn set_macos_test_permission_loss(active: bool) {
    TEST_FORCE_PERMISSION_LOSS.store(active, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn macos_test_permission_loss_active() -> bool {
    TEST_FORCE_PERMISSION_LOSS.load(Ordering::Acquire)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn observe_macos_test_physical_key(key_code: u16, held: bool) {
    if let (Some(seen), Some(state)) = (
        TEST_KEY_SEEN.get(usize::from(key_code)),
        TEST_KEY_HELD.get(usize::from(key_code)),
    ) {
        state.store(held, Ordering::Release);
        seen.store(true, Ordering::Release);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn macos_test_physical_key_state(key_code: u16) -> Option<bool> {
    let seen = TEST_KEY_SEEN.get(usize::from(key_code))?;
    seen.load(Ordering::Acquire)
        .then(|| TEST_KEY_HELD[usize::from(key_code)].load(Ordering::Acquire))
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn record_macos_test_tap_disable(confirmed: bool, reenabled: bool) {
    TEST_TAP_DISABLE_CONFIRMED.store(confirmed, Ordering::Release);
    TEST_TAP_DRAIN_REENABLED.store(reenabled, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_target_caret_identity_contract() -> bool {
    target::test_target_caret_identity_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn record_test_replay_submission() {
    let sequence = TEST_EVENT_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    TEST_REPLAY_SEQUENCE.store(sequence, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn record_test_mouse_repost() {
    let sequence = TEST_EVENT_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    TEST_MOUSE_SEQUENCE.store(sequence, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn record_test_marker_acknowledgement(
    class: MacosTestOperationClass,
    token: injection::OperationToken,
) {
    let index = class as usize;
    let generation = token.generation();
    let previous = TEST_OPERATION_GENERATIONS[index].swap(generation, Ordering::AcqRel);
    if previous != generation {
        TEST_OPERATION_OBSERVATIONS[index].store(0, Ordering::Release);
    }
    TEST_OPERATION_OBSERVATIONS[index].fetch_add(1, Ordering::AcqRel);
    TEST_MARKER_ACKS.fetch_add(1, Ordering::AcqRel);
}
