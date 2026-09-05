//! Install-scoped injection provenance and debug harness classification.
use super::*;

// The low-level injected flag is required as well as an unpredictable marker;
// a physical record can never opt into a helper class merely by carrying the
// same integer in KBDLLHOOKSTRUCT. One set is created for each hook install and
// shared by callback classification and every injection path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::windows) struct InjectionMarkers {
    pub(super) replay: usize,
    pub(super) paste: usize,
    pub(super) dummy: usize,
}

impl InjectionMarkers {
    pub(in crate::platform::windows) fn generate() -> Option<Self> {
        for _ in 0..4 {
            let mut bytes = [0_u8; size_of::<usize>()];
            // SAFETY: the system-preferred CSPRNG accepts a null algorithm handle
            // and bytes is a fully writable, correctly sized output buffer.
            let status = unsafe {
                BCryptGenRandom(
                    null_mut(),
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            };
            if status != 0 {
                return None;
            }
            let markers = Self::from_seed(usize::from_le_bytes(bytes));
            if markers.replay > 3
                && !reserved_test_marker(markers.replay)
                && !reserved_test_marker(markers.paste)
                && !reserved_test_marker(markers.dummy)
            {
                return Some(markers);
            }
        }
        None
    }

    pub(super) const fn from_seed(seed: usize) -> Self {
        let base = seed & !3;
        Self {
            replay: base | 1,
            paste: base | 2,
            dummy: base | 3,
        }
    }
}

// Deliberately absent from ordinary/release helpers. The integration harness
// duplicates this build-contract value when it emits test-only SendInput.
#[cfg(all(
    feature = "windows-native-test-input",
    debug_assertions,
    target_pointer_width = "64"
))]
pub(in crate::platform::windows) const TEST_PHYSICAL_MARKER: usize = 0x5451_5445_5354_0008;
#[cfg(all(
    feature = "windows-native-test-input",
    debug_assertions,
    target_pointer_width = "32"
))]
pub(in crate::platform::windows) const TEST_PHYSICAL_MARKER: usize = 0x5445_5308;

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
const fn reserved_test_marker(marker: usize) -> bool {
    marker == super::TEST_PHYSICAL_MARKER
}

#[cfg(not(all(feature = "windows-native-test-input", debug_assertions)))]
const fn reserved_test_marker(_marker: usize) -> bool {
    false
}

#[must_use]
pub(in crate::platform::windows) const fn classify(
    markers: InjectionMarkers,
    flags: u32,
    marker: usize,
) -> InputSource {
    if flags & LLKHF_INJECTED == 0 {
        return InputSource::Physical;
    }
    if marker == markers.replay {
        InputSource::HelperReplay
    } else if marker == markers.paste {
        InputSource::HelperPaste
    } else if marker == markers.dummy {
        InputSource::HelperDummy
    } else {
        classify_test_or_external(marker)
    }
}

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
const fn classify_test_or_external(marker: usize) -> InputSource {
    if marker == TEST_PHYSICAL_MARKER {
        InputSource::test_physical()
    } else {
        InputSource::External
    }
}

#[cfg(not(all(feature = "windows-native-test-input", debug_assertions)))]
const fn classify_test_or_external(_marker: usize) -> InputSource {
    InputSource::External
}
