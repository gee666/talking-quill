//! Process identity, operation tokens, and source/shape classification.

use super::*;

pub(super) const INJECTION_MARKER_COUNT: usize = 1;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) const TEST_PHYSICAL_MARKER: i64 = 0x5451_5048_5953_4943;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) const TEST_PERMISSION_LOSS_MARKER: i64 = TEST_PHYSICAL_MARKER + 1;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) const MACOS_TEST_SEAM_BUILD_MARKER: &[u8] =
    b"TALKING_QUILL_MACOS_NATIVE_TEST_SEAMS=ENABLED";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::macos) struct InjectionIdentity {
    pub(in crate::platform::macos) source_pid: i64,
    pub(super) process_nonce: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::macos) struct OperationToken {
    pub(super) generation: u64,
    pub(super) marker: i64,
}

impl OperationToken {
    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(in crate::platform::macos) const fn generation(self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(in crate::platform::macos) const fn marker(self) -> i64 {
        self.marker
    }

    #[cfg(test)]
    pub(in crate::platform::macos) const fn for_test(generation: u64) -> Self {
        Self {
            generation,
            marker: 0x5a5a_0000_0000_0000_u64.wrapping_add(generation) as i64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::macos) struct Submission {
    pub(in crate::platform::macos) count: usize,
    pub(in crate::platform::macos) token: Option<OperationToken>,
}

impl InjectionIdentity {
    pub(in crate::platform::macos) fn new() -> Result<Self, PlatformError> {
        #[cfg(feature = "transactional-shortcuts-dev")]
        let _ = std::hint::black_box(MACOS_TEST_SEAM_BUILD_MARKER);
        let mut markers = [0_i64; INJECTION_MARKER_COUNT];
        // SAFETY: null selects kSecRandomDefault and the fixed marker array is
        // writable for the exact byte count. This is owner startup, pre-tap.
        let status = unsafe {
            ffi::SecRandomCopyBytes(
                null_mut(),
                std::mem::size_of_val(&markers),
                markers.as_mut_ptr().cast(),
            )
        };
        if status != 0 || markers.contains(&0) {
            return Err(PlatformError::NativeFailure);
        }
        // SAFETY: getpid has no preconditions and cannot block or allocate.
        let source_pid = i64::from(unsafe { ffi::getpid() });
        if source_pid <= 0 {
            return Err(PlatformError::NativeFailure);
        }
        Ok(Self {
            source_pid,
            process_nonce: markers[0],
        })
    }

    #[cfg(test)]
    pub(in crate::platform::macos) const fn for_test(source_pid: i64) -> Self {
        Self {
            source_pid,
            process_nonce: 0x5a5a_0000_0000_0000,
        }
    }
}
#[must_use]
pub(in crate::platform::macos) const fn unmarked_source(
    identity: InjectionIdentity,
    source_pid: i64,
) -> InputSource {
    if source_pid == identity.source_pid || source_pid > 0 {
        InputSource::External
    } else {
        InputSource::Physical
    }
}

pub(in crate::platform::macos) const fn token_matches(
    identity: InjectionIdentity,
    expected: Option<OperationToken>,
    marker: i64,
    source_pid: i64,
) -> bool {
    source_pid == identity.source_pid && matches!(expected, Some(token) if token.marker == marker)
}

pub(in crate::platform::macos) const fn replay_shape_is_valid(
    event_type: u32,
    key_code: i64,
    repeat: bool,
) -> bool {
    key_code >= 0
        && key_code <= 127
        && matches!(
            event_type,
            ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
        )
        && (!repeat || event_type == ffi::K_CG_EVENT_KEY_DOWN)
}
